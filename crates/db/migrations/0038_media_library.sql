-- Media library (phase 75): a content hash so a second upload of the same
-- bytes is noticed, and a focal point (0..1 of width and height) so a
-- cropped card shows the part of the picture that matters.
ALTER TABLE media ADD COLUMN sha256 TEXT;
CREATE INDEX media_sha256_idx ON media (sha256);
ALTER TABLE media ADD COLUMN focal_x REAL;
ALTER TABLE media ADD COLUMN focal_y REAL;
