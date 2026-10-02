# Summarise `cargo test` output as "<crate>::<target> passed failed ignored".
/ Running unittests / {
    match($0, /deps\/[^)]*/); crate = substr($0, RSTART + 5, RLENGTH - 5)
    sub(/-[0-9a-f]+$/, "", crate); key = crate "::" $3
}
/ Running tests\// { key = crate "::" $2 }
/Doc-tests/ { key = "doc::" $2 }
/^test result:/ { print key, $4, $6, $8 }
