# hello (prebuilt example)

`hello-component.wasm` is a deliberately old, base-world plugin: the plugin
template as it was before the v2 exports existed. The test suite loads it to
prove that plugins built against the original contract keep working. Its
source, with the WIT of that time, is in [`source/`](source).

For a new plugin, start from [`plugin-sdk/template`](../../template) instead.

Rebuild every example component (paths are remapped, so the output carries
no local directories):

```bash
scripts/build-example-plugins.sh
```
