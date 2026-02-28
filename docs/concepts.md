# Core Concepts

## Dependencies

Tasks can specify dependencies using local IDs within the same batch:

```json
[
  {"id": "ingest-1", "name": "Ingest A", ...},
  {"id": "ingest-2", "name": "Ingest B", ...},
  {
    "id": "cluster",
    "name": "Cluster",
    "dependencies": [
      {"id": "ingest-1", "requires_success": true},
      {"id": "ingest-2", "requires_success": false}
    ],
    ...
  }
]
```

- `requires_success: true` - Parent must succeed for child to proceed
- `requires_success: false` - Parent just needs to finish (success or failure)

### Propagation Behavior

- **Parent succeeds**: Children with all dependencies met transition to `Pending`
- **Parent fails**: Children with `requires_success: true` are marked as `Failure` (cascades recursively)
- **Parent canceled**: Treated like failure - children with `requires_success: true` are marked as `Failure`

## Concurrency Rules

Control concurrent execution with rules:

```json
{
  "rules": [
    {
      "type": "Concurency",
      "matcher": {
        "kind": "data-processing",
        "status": "Running",
        "fields": ["tenant_id"]
      },
      "max_concurency": 3
    }
  ]
}
```

This limits to 3 concurrent `data-processing` tasks with the same `tenant_id` in metadata.

## Capacity Rules

Capacity rules limit total remaining work across Running (and Claimed) tasks that match the same `kind` and metadata `fields`. Remaining work is computed as `max(coalesce(expected_count, 0) - success - failures, 0)`; tasks without `expected_count` contribute `0`. Tasks that use Capacity rules must set `expected_count`, and `matcher.status` must be `Running`.

```json
{
  "rules": [
    {
      "type": "Capacity",
      "matcher": {
        "kind": "data-processing",
        "status": "Running",
        "fields": ["tenant_id"]
      },
      "max_capacity": 500
    }
  ],
  "expected_count": 1000
}
```

## Deduplication

Skip creating duplicate tasks based on metadata fields:

```json
{
  "dedupe_strategy": [
    {
      "kind": "data-processing",
      "status": "Pending",
      "fields": ["project_id"]
    }
  ]
}
```

If a task with the same `kind`, `status`, and matching `project_id` exists, the new task is skipped.
