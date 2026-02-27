#[allow(unused)]
mod assertions;
#[allow(unused)]
mod builders;
#[allow(unused)]
mod mock_server;
#[allow(unused)]
mod setup;
#[allow(unused)]
mod state;

#[allow(unused)]
pub use assertions::*;
#[allow(unused)]
pub use builders::*;
#[allow(unused)]
pub use mock_server::*;
#[allow(unused)]
pub use setup::*;
#[allow(unused)]
pub use state::*;

/// Macro to create the test actix-web service from an AppState.
/// Usage: `let app = test_service!(state);`
macro_rules! test_service {
    ($state:expr) => {
        actix_web::test::init_service(
            actix_web::App::new()
                .app_data(actix_web::web::Data::new($state.clone()))
                .configure(arcrun::handlers::configure_routes),
        )
        .await
    };
}
