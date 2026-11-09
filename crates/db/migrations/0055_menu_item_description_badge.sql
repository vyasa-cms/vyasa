-- 0055_menu_item_description_badge: an optional one-line description and a short badge on a menu item
-- (a "Products" menu entry reads "Engine -- batch and streaming  [0.2]"). Both are empty by default, so
-- every existing item renders as before.

ALTER TABLE menu_items
    ADD COLUMN description TEXT NOT NULL DEFAULT '',
    ADD COLUMN badge       TEXT NOT NULL DEFAULT '';
