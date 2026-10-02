-- 0049_custom_roles: administrator-defined roles. A role is a name and a
-- set of the existing capability names; the five built-in roles stay in
-- users.role and are not rows here.
--
-- Safe on a live database: a new table, and a nullable column with no
-- default (no table rewrite). Every existing row has custom_role NULL, so
-- the foreign key and the CHECK validate with one read of users.

CREATE TABLE roles (
    slug         TEXT PRIMARY KEY
                 CHECK (slug ~ '^[a-z0-9][a-z0-9-]{1,39}$'
                        AND slug NOT IN ('admin', 'editor', 'author', 'contributor', 'subscriber')),
    name         TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 60),
    description  TEXT NOT NULL DEFAULT '',
    -- No NULL element: every user fetch reads this list as text values, and
    -- one NULL would fail the fetch for each user holding the role.
    capabilities TEXT[] NOT NULL DEFAULT '{}'
                 CONSTRAINT roles_capabilities_no_null_check
                 CHECK (array_position(capabilities, NULL) IS NULL),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE users
    ADD COLUMN custom_role TEXT
        REFERENCES roles (slug) ON UPDATE CASCADE ON DELETE RESTRICT;

-- A user with a custom role has the least built-in role underneath it, so
-- nothing that asks "is this an administrator" is ever answered by a
-- custom role.
ALTER TABLE users
    ADD CONSTRAINT users_custom_role_base_check
        CHECK (custom_role IS NULL OR role = 'subscriber');

-- Counting and finding a role's users (list, in-use delete, FK checks).
CREATE INDEX users_custom_role_idx ON users (custom_role) WHERE custom_role IS NOT NULL;
