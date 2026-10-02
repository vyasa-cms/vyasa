# Release Checklist

1. Every change since the last release has a CHANGELOG.md entry under Unreleased
2. cargo fmt/clippy/test green
3. admin lint/typecheck/test/build green
4. cargo audit clean
5. CHANGELOG.md updated
6. Version bumped in workspace Cargo.toml
7. Docker image builds + smoke test passes
8. Migration tested against copy of production data
