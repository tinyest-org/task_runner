# Audit Codebase Codex

## Contexte
Audit statique rapide orienté bugs/risques. Aucun test n’a été exécuté.

## Constats (triés par sévérité)
1. **[High] Exécution d’actions potentiellement en double.** Le worker exécute les actions (`start_task`) avant d’avoir “claim” la tâche en `Running`. Avec plusieurs workers/instances, une même tâche peut être traitée deux fois et déclencher des webhooks en double. Références: `src/workers.rs:96`, `src/workers.rs:115`, `src/workers.rs:121`.
2. **[High] Propagation des dépendances et actions de fin rejouées.** `update_running_task` peut être appelé plusieurs fois sans vérifier l’état courant (il bloque seulement `Failure`) et relance `end_task` à chaque update. Cela peut décrémenter `wait_finished` / `wait_success` plusieurs fois et rendre les compteurs négatifs, débloquant ou bloquant incorrectement des tâches enfants. Références: `src/db_operation.rs:123`, `src/db_operation.rs:144`, `src/workers.rs:313`, `src/workers.rs:394`.
3. **[High] Endpoint d’update non validé + mapping d’erreur trompeur.** `update_task` n’appelle pas `validate_update_task`. Les valeurs invalides (status interdit, compteurs négatifs, failure_reason incohérent) peuvent passer, et les erreurs sont renvoyées comme `404 Not Found` (code 2). Références: `src/handlers.rs:219`, `src/handlers.rs:233`, `src/db_operation.rs:99`, `src/db_operation.rs:111`.
4. **[Medium] Panique si `POOL_ACQUIRE_RETRIES=0`.** Boucle `0..0` ⇒ `last_error` reste `None` puis `unwrap()`. Références: `src/handlers.rs:59`, `src/handlers.rs:84`.
5. **[Medium] Panique possible sur métadonnées manquantes.** `unreachable!` si un champ demandé par une règle/concurrency matcher n’existe pas dans `metadata`. Rien ne valide cette précondition. Références: `src/workers.rs:202`, `src/workers.rs:208`, `src/db_operation.rs:288`, `src/db_operation.rs:295`.
6. **[Medium] Dedupe sur metadata `None` peut filtrer trop large.** Si `metadata` est `None`, le matcher devient `{}` ⇒ `metadata.contains({})` est vrai pour tout, donc déduplication globale par `kind/status`. Références: `src/db_operation.rs:287`, `src/db_operation.rs:303`.
7. **[Medium] Config runtime non appliquée.** Le pool ignore `Config.pool` (taille hard‑codée), et les intervals workers/batch sont en dur, donc `Config.worker` n’a pas d’effet. Cela fausse aussi `readiness_check` qui utilise `config.pool.max_size`. Références: `src/lib.rs:37`, `src/lib.rs:43`, `src/workers.rs:78`, `src/workers.rs:171`, `src/workers.rs:511`.

## Questions / hypothèses
1. Le service doit‑il être safe en multi‑instance (plusieurs workers/replicas) ? Si oui, il faut un “claim” atomique (UPDATE … WHERE status=Pending RETURNING … ou `SELECT … FOR UPDATE SKIP LOCKED`) avant d’exécuter les actions.
2. L’API `PATCH /task/{id}` est‑elle exposée publiquement ? Si oui, la validation doit être appliquée strictement et les erreurs doivent être `400`.
3. La déduplication doit‑elle exiger que toutes les `matcher.fields` existent dans `metadata` ? Aujourd’hui, ça panique si absent.

## Tests / gaps
1. Aucun test n’a été exécuté.
2. Gaps évidents: tests concurrence (double exécution), tests d’updates répétées (propagation et compteurs), tests de validation update et dedupe sur metadata absente.
