-- Add index on link.parent_id for propagate_to_children lookups.
-- The composite PK (parent_id, child_id) can serve parent_id queries,
-- but a dedicated single-column index is smaller and preferred by the planner
-- when only parent_id is filtered.
CREATE INDEX idx_link_parent_id ON link(parent_id);
