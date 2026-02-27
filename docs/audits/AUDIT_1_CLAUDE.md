# Audit de la codebase arcrun

Date : 2026-02-14

---

## CRITIQUE

### 1. `end_task` ne filtre pas par `condition` — les webhooks success ET failure sont toujours exécutés

**Fichier** : `src/workers.rs` — fonction `end_task`

`end_task` charge les actions avec `trigger.eq(TriggerKind::End)` mais ne vérifie jamais le champ `condition` (`TriggerCondition::Success` / `TriggerCondition::Failure`). Résultat : quand une tâche réussit, les actions `on_failure` sont aussi exécutées, et inversement.

**Impact** : Comportement fonctionnel incorrect sur chaque fin de tâche.

**Fix** : Ajouter un filtre `.filter(condition.eq(...))` basé sur le `result_status` passé en paramètre.

---

### 2. `unreachable!()` et `.expect()` dans du code runtime — crash du serveur

| Localisation | Pattern | Contexte |
|---|---|---|
| `src/db_operation.rs:295` | `unreachable!("None shouldn't be there")` | `handle_dedupe` — champ metadata manquant dans un matcher |
| `src/workers.rs:208` | `unreachable!("None shouldn't be there")` | `evaluate_rules` — champ metadata manquant dans une règle de concurrence |
| `src/db_operation.rs:308` | `.expect("failed to count for execution")` | `handle_dedupe` — erreur DB lors du comptage |
| `src/workers.rs:220` | `.expect("failed to count for execution")` | `evaluate_rules` — erreur DB lors du comptage |

**Impact** : Un input utilisateur malformé (metadata incomplète) ou une erreur DB transitoire crash le process entier. Le worker loop s'arrête pour toutes les tâches.

**Fix** : Remplacer par des `Result` propres. Retourner une erreur ou skip la tâche concernée.

---

### 3. Webhook `on_start` tiré AVANT la mise à jour en DB

**Fichier** : `src/workers.rs` — `start_loop`

Le worker appelle `start_task()` (webhook) puis `set_started_task()` (DB update). Séquence problématique :

1. Si le DB update échoue après le webhook, la tâche reste `Pending` et le webhook sera ré-exécuté au prochain cycle (boucle infinie sans backoff).
2. Si le webhook échoue, l'erreur est loggée mais la tâche reste `Pending` — retry infini sans limite.
3. En cas de scaling horizontal, deux workers peuvent fetch la même tâche `Pending` et tirer le webhook deux fois (pas de `SELECT FOR UPDATE SKIP LOCKED`).

**Impact** : Webhooks dupliqués, boucle de retry infinie, effets de bord multiples sur le système externe.

**Fix** : Marquer la tâche `Running` en DB d'abord (avec `WHERE status = 'pending'` atomique), puis tirer le webhook. Si le webhook échoue, marquer en `Failure` ou retry avec backoff.

---

## HIGH

### 4. `cancel_task` peut écraser un état terminal (Success/Failure -> Canceled)

**Fichier** : `src/workers.rs:480`

```rust
diesel::update(task.filter(id.eq(task_id)))
    .set((status.eq(StatusKind::Canceled), ended_at.eq(now)))
    .execute(conn).await?;
```

Le `UPDATE` n'a pas de filtre sur le status actuel. Entre le `SELECT` (ligne 445) et l'`UPDATE`, la tâche pourrait avoir transitionné vers `Success` ou `Failure` via `update_running_task`. Le cancel écrase silencieusement un état terminal.

**Impact** : Une tâche réussie peut être marquée comme annulée, et la propagation d'annulation se déclenche sur ses enfants.

**Fix** : Ajouter `.filter(status.eq_any([StatusKind::Pending, StatusKind::Running, StatusKind::Paused]))` au `UPDATE`.

---

### 5. Double-propagation possible sur `update_running_task`

**Fichier** : `src/db_operation.rs:123-142`

Le filtre est `status.ne(StatusKind::Failure)`. Deux requêtes HTTP concurrentes envoyant `Success` passent toutes les deux le filtre. Chacune appelle `end_task` -> `propagate_to_children`. Les compteurs `wait_finished` des enfants sont décrémentés deux fois.

**Impact** : `wait_finished` peut devenir négatif. L'enfant ne transitionne jamais vers `Pending` (le check est `wait_finished.eq(0)`). La tâche enfant reste bloquée en `Waiting` pour toujours.

**Fix** : Changer le filtre en `status.eq(StatusKind::Running)` et vérifier que `res == 1` avant d'appeler `end_task`.

---

### 6. Configuration du pool de connexions ignorée

**Fichier** : `src/lib.rs:37-48`

Les paramètres du pool sont hardcodés :

```rust
Pool::builder()
    .max_size(10)
    .min_idle(Some(5))
    .max_lifetime(Some(Duration::from_secs(60 * 60 * 24)))
    .idle_timeout(Some(Duration::from_secs(60 * 2)))
```

Les valeurs de `PoolConfig` chargées depuis les variables d'environnement (`POOL_MAX_SIZE`, `POOL_MIN_IDLE`, etc.) ne sont jamais utilisées.

**Impact** : Impossible de tuner le pool en production sans recompiler.

**Fix** : Passer `Config` ou `PoolConfig` à `initialize_db_pool()` et utiliser les valeurs.

---

### 7. `validate_update_task` n'est jamais appelée

**Fichier** : `src/validation.rs`

La fonction `validate_update_task` existe mais est du dead code. Ni `update_task` ni `batch_task_updater` ne l'appellent.

**Impact** : `batch_task_updater` accepte des valeurs `new_success: -100` sans validation. Les compteurs de tâches peuvent devenir négatifs.

**Fix** : Appeler `validate_update_task` dans les handlers `update_task` et `batch_task_updater`.

---

### 8. `BLOCKED_HOSTNAMES` custom remplace les défauts — bypass SSRF

**Fichier** : `src/config.rs:326-328`

Définir `BLOCKED_HOSTNAMES=myhost.com` supprime `localhost`, `127.0.0.1`, `metadata.google.internal`, etc. de la blocklist. La vérification IP hardcodée (`is_internal_ip`) couvre les plages IP internes, mais les hostnames comme `http://localhost/` passeraient.

**Impact** : Risque de SSRF si un opérateur configure `BLOCKED_HOSTNAMES` sans réaliser qu'il supprime les défauts.

**Fix** : Fusionner les hostnames custom avec les défauts au lieu de les remplacer, ou documenter clairement le comportement.

---

## MEDIUM

### 9. Pas de transaction autour de la propagation

**Fichiers** : `src/db_operation.rs`, `src/workers.rs`

`update_running_task` -> `end_task` -> `propagate_to_children` ne sont pas wrappés dans une transaction. Chaque `diesel::update` est auto-commité.

- Si le process crash entre le décrémentation du compteur enfant et la transition de status, l'enfant reste bloqué en `Waiting` avec `wait_finished=0` pour toujours.
- Si `end_task` échoue après le `UPDATE` du parent, le parent est marqué Success/Failure mais les enfants ne sont jamais notifiés.

**Impact** : Tâches enfants orphelines en cas de crash ou d'erreur DB transitoire.

---

### 10. `pause_task` sans vérification d'état

**Fichier** : `src/db_operation.rs:523-533`

```rust
diesel::update(task.filter(id.eq(task_id)))
    .set(status.eq(StatusKind::Paused))
    .execute(conn).await?;
```

Aucun filtre sur le status actuel. Une tâche `Success`, `Failure`, ou `Canceled` peut être remise en `Paused`.

**Impact** : Violation de la machine à états. Comportement indéfini si le worker rencontre une tâche en `Paused` qui était précédemment terminée.

**Fix** : Ajouter `.filter(status.eq_any([StatusKind::Pending, StatusKind::Running]))`.

---

### 11. Transaction manuelle fragile dans `add_task`

**Fichier** : `src/handlers.rs:322-375`

`BEGIN`/`COMMIT`/`ROLLBACK` via `diesel::sql_query()` raw. Problèmes :

- Si le code panic entre `BEGIN` et `COMMIT`, la connexion retourne au pool avec une transaction ouverte. Les requêtes suivantes sur cette connexion opèrent dans la transaction stale.
- Les `ROLLBACK` utilisent `let _ = ...` qui ignore silencieusement les erreurs de rollback.

**Fix** : Utiliser le support transactionnel de diesel (`conn.transaction(...)`) ou au minimum un guard RAII.

---

### 12. Race condition dans le batch updater (DashMap `retain`)

**Fichier** : `src/workers.rs:579-582`

Le `retain` supprime les entrées où les deux atomiques sont à 0. Mais entre la lecture des atomiques dans `retain` et la suppression effective de l'entrée, le receiver peut ajouter de nouveaux compteurs.

Séquence :
1. `retain` lit `success=0, failures=0` pour la tâche X
2. Le receiver ajoute `success=1` pour la tâche X
3. `retain` supprime l'entrée
4. Le `success=1` est perdu

**Impact** : Perte de données de compteurs sous charge.

---

### 13. Timeout basé sur `last_updated` au lieu de `started_at`

**Fichier** : `src/db_operation.rs:64`

La requête de timeout vérifie `last_updated.lt(now - timeout)`. Les mises à jour de compteurs via le batch updater (`handle_one_with_counts`) rafraîchissent `last_updated`. Une tâche qui reçoit des updates périodiques ne timeout jamais, même si elle tourne depuis plus longtemps que son timeout.

**Impact** : Des tâches stuck qui reçoivent des heartbeats ne sont jamais nettoyées.

---

### 14. `update_running_task` retourne `Ok(2)` pour les erreurs de validation

**Fichier** : `src/db_operation.rs:102,112`

Magic number `Ok(2)` pour signaler un input invalide. Le handler traite tout `!= 1` comme "Not Found" :

```rust
Ok(match count {
    1 => HttpResponse::Ok().body("Task updated successfully"),
    _ => HttpResponse::NotFound().body("Task not found"),
})
```

**Impact** : Les erreurs de validation (status invalide, failure_reason sans status Failure) retournent 404 au lieu de 400 Bad Request.

**Fix** : Utiliser un type d'erreur dédié au lieu d'un magic number.

---

### 15. Nouveau `reqwest::Client` à chaque webhook

**Fichier** : `src/action.rs:67`

`get_http_client()` crée un nouveau `reqwest::Client` pour chaque exécution de webhook. Chaque client a son propre pool de connexions TCP/TLS.

**Impact** : Pas de réutilisation des connexions, overhead de handshake TLS répété, consommation mémoire sous charge.

**Fix** : Stocker un `reqwest::Client` partagé dans `ActionExecutor` ou en `static`.

---

### 16. Pas de shutdown gracieux pour les worker loops

**Fichier** : `src/workers.rs`

`start_loop`, `timeout_loop`, et `batch_updater` sont des `loop {}` infinis sans signal d'arrêt (pas de `CancellationToken` ou `watch` channel).

**Impact** : À l'arrêt du serveur (SIGTERM), le batch updater peut perdre des compteurs accumulés dans le DashMap qui n'ont pas encore été persistés en DB.

**Fix** : Utiliser un `tokio_util::sync::CancellationToken` ou `tokio::sync::watch` pour coordonner l'arrêt.

---

### 17. Pas de mécanisme de rétention/nettoyage des données

Les tables `task`, `action`, `link` croissent sans limite. Il n'y a pas de TTL, pas de job de cleanup, pas de `ON DELETE CASCADE` sur les foreign keys.

**Impact** : Croissance illimitée de la DB en production.

---

### 18. Double `entry().or_default()` dans le receiver DashMap

**Fichier** : `src/workers.rs:521-528`

```rust
data.entry(evt.task_id).or_default().success.fetch_add(evt.success, Ordering::Relaxed);
data.entry(evt.task_id).or_default().failures.fetch_add(evt.failures, Ordering::Relaxed);
```

Deux lookups séparés pour le même `task_id`. Entre les deux, `retain` peut supprimer l'entrée. Le `success` du premier appel est perdu, seul le `failures` du second est conservé dans une nouvelle entrée.

Le même problème existe dans le chemin de re-queue (lignes 566-573).

**Fix** : Un seul `entry()` pour les deux opérations :
```rust
let entry = data.entry(evt.task_id).or_default();
entry.success.fetch_add(evt.success, Ordering::Relaxed);
entry.failures.fetch_add(evt.failures, Ordering::Relaxed);
```

---

## LOW

### 19. `Ordering::Relaxed` partout dans le batch updater

**Fichier** : `src/workers.rs`

Toutes les opérations atomiques utilisent `Relaxed`. Sur ARM, un `swap(0)` Relaxed combiné avec un `fetch_add` Relaxed peut théoriquement perdre une mise à jour. Sur x86, le risque est nul (ordering fort).

**Fix** : Utiliser `AcqRel` pour `swap` et `Release` pour `fetch_add`.

---

### 20. LIKE filter sans échappement de `%` et `_`

**Fichier** : `src/db_operation.rs:204-205`

```rust
let name_filter = name.like(format!("%{}%", filter.name.unwrap_or("".to_string())));
```

Les caractères spéciaux LIKE (`%`, `_`) dans l'input utilisateur ne sont pas échappés. Ce n'est pas de l'injection SQL (Diesel paramétrise), mais les patterns de matching sont altérés.

---

### 21. Health check leak des erreurs DB internes

**Fichier** : `src/handlers.rs:111`

```rust
format!("unhealthy: {}", e)
```

La réponse du health check inclut le message d'erreur brut de la DB, potentiellement des détails de connexion ou des hostnames internes.

---

### 22. `cancel_task`/`pause_task` retournent 400 pour toutes les erreurs

**Fichier** : `src/handlers.rs:407-429`

Toutes les erreurs (y compris les erreurs DB, problèmes de connexion) sont mappées en `HttpResponse::BadRequest()`. Une panne DB retourne 400 au lieu de 500.

---

### 23. `once_cell` redondant avec Rust 2024 edition

**Fichier** : `Cargo.toml`

`once_cell::sync::Lazy` peut être remplacé par `std::sync::LazyLock` (stable depuis Rust 1.80).

---

### 24. `diesel-derive-enum` en version beta `3.0.0-beta.1`

**Fichier** : `Cargo.toml`

Dépendance beta en production. L'API peut changer dans la release finale.

---

### 25. Pas de limite sur la taille des batch de création de tâches

**Fichier** : `src/handlers.rs`

`add_task` accepte `Vec<NewTaskDto>` sans limite. Un client peut soumettre des millions de tâches en une seule requête.

---

### 26. Erreurs de `end_task` silencieusement avalées

**Fichier** : `src/db_operation.rs:155-158`

```rust
match end_task(evaluator, &task_id, dto.status.clone().unwrap(), conn).await {
    Ok(_) => log::debug!("task {} end actions are successfull", &task_id),
    Err(_) => log::error!("task {} end actions failed", &task_id),
}
```

Si `end_task` échoue (webhooks + propagation), l'erreur est loggée mais `update_running_task` retourne `Ok(res)`. Le client reçoit "Task updated successfully" alors que les enfants n'ont pas été propagés.

---

### 27. Status des actions (`success: Option<bool>`) jamais mis à jour

**Fichier** : `src/models.rs`

Le champ `Action.success` n'est jamais écrit après insertion. Les commentaires dans `start_task` et `end_task` mentionnent "update the action status" mais le code ne le fait pas. Impossible d'auditer quelles actions ont réussi ou échoué.

---

### 28. Worker intervals hardcodés au lieu d'utiliser la config

**Fichier** : `src/workers.rs`

Les boucles utilisent `sleep_secs(1)` et `sleep_ms(100)` en dur. Les valeurs `WORKER_LOOP_INTERVAL_MS` et `TIMEOUT_CHECK_INTERVAL_MS` de la config sont chargées mais jamais lues par les workers.

---

### 29. `GLOBAL_SENDER`/`GLOBAL_RECEIVER` déclarés mais jamais utilisés

**Fichier** : `src/workers.rs:23-27`

```rust
#[allow(dead_code)]
pub(crate) static GLOBAL_SENDER: OnceCell<mpsc::Sender<UpdateEvent>> = OnceCell::const_new();
#[allow(dead_code)]
pub(crate) static GLOBAL_RECEIVER: OnceCell<Mutex<mpsc::Receiver<UpdateEvent>>> = OnceCell::const_new();
```

Dead code explicitement supprimé des warnings. A supprimer.

---

### 30. Metrics enregistrées même quand 0 rows updated

**Fichier** : `src/db_operation.rs:144-152`

`record_status_transition` et `record_task_completed` sont appelés quand `dto.status` est `Some`, sans vérifier que `res > 0`. Si aucune tâche n'a été modifiée, les métriques sont faussées.

---

### 31. `TASKS_BY_STATUS` gauge utilisé pour compter les pool exhaustions

**Fichier** : `src/handlers.rs:90-92`

```rust
metrics::TASKS_BY_STATUS.with_label_values(&["pool_exhausted"]).inc();
```

Un gauge (valeur courante) est utilisé comme compteur monotone avec un label sémantiquement incorrect (`pool_exhausted` n'est pas un status de tâche). Devrait être un counter séparé.

---

### 32. Down migration pour `add-cancel-status` est vide

**Fichier** : `migrations/2025-08-27-091353_add-cancel-status/down.sql`

PostgreSQL ne supporte pas `ALTER TYPE ... REMOVE VALUE`. La migration est irréversible sans workaround.

---

### 33. DNS rebinding bypass potentiel sur la validation SSRF

**Fichier** : `src/validation.rs`

La validation SSRF vérifie le hostname au moment de la validation, mais le webhook est exécuté plus tard. Un attaquant pourrait utiliser du DNS rebinding pour résoudre vers une IP interne au moment de l'exécution.

---

### 34. IPv4-mapped IPv6 bypass potentiel sur la validation SSRF

**Fichier** : `src/validation.rs`

`is_internal_ip` vérifie IPv4 et IPv6 séparément mais ne couvre pas les adresses IPv4-mapped IPv6 (`::ffff:127.0.0.1`).

---

## Statistique

| Sévérité | Nombre |
|----------|--------|
| Critique | 3 |
| High | 5 |
| Medium | 10 |
| Low | 16 |
| **Total** | **34** |
