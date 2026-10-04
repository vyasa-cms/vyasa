-- The marketplace and update channel are compiled into the binary now
-- (crates/api/src/official.rs); an operator mirrors or disables them in
-- vyasa.toml. The options that used to choose them would only mislead.
DELETE FROM options
WHERE key IN ('registry_url', 'registry_trusted_keys', 'update_channel_url', 'update_trusted_keys');
