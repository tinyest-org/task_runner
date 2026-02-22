# Plan Idempotency Key

## Objectif
Garantir que les webhooks (`on_start`, `on_success`, `on_failure`, `on_cancel`) soient idempotents même en cas de crash, retry, ou requeue des tâches `Claimed`.

## Principes
- La clé d'idempotence doit être **déterministe** et **stable** pour un même événement.
- Le serveur doit **envoyer** la clé à la cible (header HTTP) et **enregistrer** localement l'exécution pour éviter les doublons.
- Le consommateur du webhook doit pouvoir **dédupliquer** côté serveur (recommandé), mais le task-runner doit aussi empêcher les doubles envois autant que possible.

## Spécification proposée
### Clé d'idempotence
- Format recommandé: `task_id:trigger[:attempt]`
  - `task_id` = UUID de la tâche
  - `trigger` = `start|success|failure|cancel`
  - `attempt` optionnel si on introduit plus tard des retries explicites
- Exemple: `c2b1...-uuid:start`
  
**Note condition**: pour `start` et `cancel`, la colonne `condition` reste **non nulle**
et utilise le sentinel `Success` (la condition n'a pas de sens pour ces triggers).
La clé d'idempotence ignore `condition` pour `start`/`cancel`.

### Transport
- Header HTTP: `Idempotency-Key: <value>`
- Optionnel: ajouter aussi `X-Task-Id` et `X-Task-Trigger` pour diagnostics

## Plan d'implémentation
### 1) Modèle de données (persistance des exécutions)
**Option A (simple, recommandé)**: table dédiée `webhook_execution`
- Colonnes:
  - `id` UUID PK
  - `task_id` UUID FK
  - `trigger` ENUM (`start`, `end`, `cancel`)
  - `condition` ENUM (`success`, `failure`) **(sentinel `success` pour `start`/`cancel`)**
  - `idempotency_key` TEXT UNIQUE
  - `status` ENUM (`pending`, `success`, `failure`)
  - `attempts` INT
  - `created_at`, `updated_at`

**Option B (réutiliser action)**
- Ajouter `idempotency_key` + contrainte UNIQUE sur `(task_id, trigger, condition)`.
- Moins propre car une action peut être multiple et ce champ ne représente pas une exécution, mais acceptable si chaque action est unique.

### 2) Génération de la clé
- Ajouter fonction utilitaire dans `src/action.rs`:
  - `fn idempotency_key(task_id: Uuid, trigger: TriggerKind, condition: Option<TriggerCondition>) -> String`
  - `condition` = `None` pour `start/cancel` ; `Success|Failure` pour `end`

### 3) Émission des webhooks
- Avant l'appel HTTP:
  1. Calculer la clé.
  2. Tenter d'insérer un enregistrement `webhook_execution` avec `status=pending`.
  3. Si insertion échoue (conflit UNIQUE) => considérer l'exécution déjà faite, **ne pas réémettre**.
- Après réponse HTTP:
  - Mettre `status=success` ou `failure` + `attempts += 1`.

### 4) Gestion des retries
- Pas de retry automatique immédiat: `pending` bloque les doublons.
- Retry autorisé si `status=failure`.
- Retry autorisé si `status=pending` **et** l'exécution est considérée "stale"
  (timeout configuré, même durée que `claim_timeout`).

### 5) Observabilité
- Metrics:
  - `webhook_idempotent_skips_total{trigger}`
  - `webhook_idempotent_conflicts_total`
  - `webhook_attempts_total{trigger,outcome}`
- Logs: inclure `idempotency_key` systématiquement.

### 6) Documentation API
- Documenter le header `Idempotency-Key` dans `readme.md` + OpenAPI (`src/handlers.rs` description webhook).
- Exemple client: ignorer doublons côté consumer.

## Étapes d'exécution
1. Ajouter la migration (table `webhook_execution` + index UNIQUE).
2. Implémenter le modèle + CRUD dans `src/db_operation.rs`.
3. Ajouter la génération de la clé dans `src/action.rs` et l'injecter en header.
4. Mettre à jour le flow d'exécution des actions (`start_task`, `end_task`, `fire_end_webhooks`, `cancel_task`) pour écrire/consulter la table avant envoi.
5. Ajouter metrics + logs.
6. Mettre à jour la doc.
7. Ajouter tests:
   - double exécution => 1 seul envoi
   - requeue `Claimed` => pas de double `on_start`

## Risques / points d'attention
- Concurrence: l'unicité en DB doit être **le garde-fou principal**.
- Compatibilité: le consumer doit accepter ce header sans erreur (aucun effet de bord).
- Migration: attention aux environnements existants (backfill non requis si table dédiée).
