-- 0015_menus: navigation menus and their ordered, nestable items.

CREATE TABLE menus (
    id          BIGINT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    slug        TEXT NOT NULL UNIQUE,
    location    TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE menu_items (
    id          BIGINT PRIMARY KEY,
    menu_id     BIGINT NOT NULL REFERENCES menus (id) ON DELETE CASCADE,
    parent_id   BIGINT REFERENCES menu_items (id) ON DELETE CASCADE,
    label       TEXT NOT NULL,
    url         TEXT NOT NULL,
    sort_order  INTEGER NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX menu_items_menu_idx ON menu_items (menu_id, sort_order);
