use diesel::{Connection, PgConnection, RunQueryDsl};
use diesel_async::AsyncPgConnection;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::bb8::Pool;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use task_runner::DbPool;
use testcontainers::{ImageExt, runners::AsyncRunner};
use testcontainers_modules::postgres::Postgres;

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

/// Test application state that owns the PostgreSQL container.
/// The container is kept alive for the duration of the test via RAII.
pub struct TestApp {
    pub pool: DbPool,
    pub _container: testcontainers::ContainerAsync<Postgres>,
}

/// Setup a test database and return the pool
pub async fn setup_test_db() -> TestApp {
    let container = Postgres::default()
        .with_tag("18-alpine")
        .start()
        .await
        .unwrap();
    let host_port = container.get_host_port_ipv4(5432).await.unwrap();

    let database_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres",
        host_port
    );

    // Run migrations using sync connection
    run_migrations(&database_url);

    // Create async pool
    let config = AsyncDieselConnectionManager::<AsyncPgConnection>::new(&database_url);
    let pool = Pool::builder()
        .max_size(5)
        .build(config)
        .await
        .expect("Failed to create pool");

    TestApp {
        pool,
        _container: container,
    }
}

/// Convert ALTER TYPE ... ADD VALUE to use IF NOT EXISTS for idempotency
fn make_alter_type_idempotent(statement: &str) -> String {
    let upper = statement.to_uppercase();
    if upper.contains("IF NOT EXISTS") {
        // Already idempotent
        statement.to_string()
    } else {
        // Insert IF NOT EXISTS after ADD VALUE
        // Pattern: ALTER TYPE <name> ADD VALUE '<value>'
        // Convert to: ALTER TYPE <name> ADD VALUE IF NOT EXISTS '<value>'
        let add_value_pos = upper.find("ADD VALUE").unwrap_or(0);
        if add_value_pos > 0 {
            let before = &statement[..add_value_pos + 9]; // "ADD VALUE"
            let after = &statement[add_value_pos + 9..];
            format!("{} IF NOT EXISTS{}", before, after)
        } else {
            statement.to_string()
        }
    }
}

/// Run migrations, handling ALTER TYPE ... ADD VALUE specially
///
/// PostgreSQL's ALTER TYPE ... ADD VALUE cannot run inside a transaction block.
/// This function detects such migrations by reading their SQL content and runs
/// them outside of transactions.
fn run_migrations(database_url: &str) {
    use std::fs;
    use std::path::Path;

    let mut conn = {
        let max_retries: u64 = 10;
        let mut result = PgConnection::establish(database_url);
        for attempt in 1..max_retries {
            if result.is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200 * attempt));
            result = PgConnection::establish(database_url);
        }
        result.expect("Failed to connect for migrations after retries")
    };

    let pending = conn
        .pending_migrations(MIGRATIONS)
        .expect("Failed to get pending migrations");

    // Collect migration info first
    let migrations_to_run: Vec<_> = pending
        .iter()
        .map(|m| (m.name().to_string(), m.name().version().to_string()))
        .collect();

    // Build a map of migrations that contain ALTER TYPE ... ADD VALUE
    // by reading the actual migration SQL files
    let migrations_dir = Path::new("migrations");
    let alter_type_migrations: std::collections::HashMap<String, Vec<String>> =
        if migrations_dir.exists() {
            fs::read_dir(migrations_dir)
                .ok()
                .map(|entries| {
                    entries
                        .flatten()
                        .filter_map(|entry| {
                            let dir_name = entry.file_name().to_string_lossy().to_string();
                            let up_sql_path = entry.path().join("up.sql");

                            if up_sql_path.exists() {
                                let content = fs::read_to_string(&up_sql_path).ok()?;

                                // Find all ALTER TYPE ... ADD VALUE statements
                                let statements: Vec<String> = content
                                    .lines()
                                    .filter(|line| {
                                        let upper = line.to_uppercase();
                                        upper.contains("ALTER TYPE") && upper.contains("ADD VALUE")
                                    })
                                    .map(|s| make_alter_type_idempotent(s.trim()))
                                    .collect();

                                if !statements.is_empty() {
                                    return Some((dir_name, statements));
                                }
                            }
                            None
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            std::collections::HashMap::new()
        };

    // Run each migration
    for (migration_name, version) in &migrations_to_run {
        // Check if this migration contains ALTER TYPE ... ADD VALUE
        // by looking for matching directory name in our map
        let alter_statements: Option<&Vec<String>> = alter_type_migrations
            .iter()
            .find(|(dir_name, _)| migration_name.contains(dir_name.as_str()))
            .map(|(_, stmts)| stmts);

        if let Some(statements) = alter_statements {
            // This migration has ALTER TYPE ... ADD VALUE statements
            // Run them outside of a transaction

            // Ensure no transaction is active by reconnecting
            drop(conn);
            conn = PgConnection::establish(database_url)
                .expect("Failed to reconnect for ALTER TYPE migration");

            // Run each ALTER TYPE statement
            for stmt in statements {
                diesel::sql_query(stmt.clone())
                    .execute(&mut conn)
                    .unwrap_or_else(|e| {
                        // Log but don't fail - value might already exist in some PostgreSQL versions
                        eprintln!("Warning: ALTER TYPE statement may have failed (possibly already exists): {}", e);
                        0
                    });
            }

            // Mark migration as run in diesel's schema_migrations table
            diesel::sql_query(format!(
                "INSERT INTO __diesel_schema_migrations (version, run_on) VALUES ('{}', NOW()) ON CONFLICT DO NOTHING",
                version
            ))
            .execute(&mut conn)
            .expect("Failed to record migration");
        } else {
            // Run migration normally within a transaction
            let pending_now = conn
                .pending_migrations(MIGRATIONS)
                .expect("Failed to get pending migrations");

            for migration in pending_now {
                if migration.name().to_string() == *migration_name {
                    let _ = diesel::sql_query("COMMIT").execute(&mut conn);
                    conn.run_migration(&migration)
                        .expect(&format!("Failed to run migration: {}", migration_name));
                    break;
                }
            }
        }
    }
}
