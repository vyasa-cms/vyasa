-- Bundled theme files (phase 70): the pictures and fonts a .vytheme ships
-- under assets/images/ and assets/fonts/. One row per file per theme
-- version; a version copies its parent's rows when the studio publishes,
-- and the rows go with the version when it is deleted. Kept out of the
-- themes row so the render path, which reads that row on every page,
-- never loads a byte of them.
CREATE TABLE theme_files (
    theme_id     BIGINT NOT NULL REFERENCES themes (id) ON DELETE CASCADE,
    path         TEXT   NOT NULL,
    content_type TEXT   NOT NULL,
    sha256       TEXT   NOT NULL,
    bytes        BYTEA  NOT NULL,
    PRIMARY KEY (theme_id, path)
);
