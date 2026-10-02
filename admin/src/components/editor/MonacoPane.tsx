import * as React from "react";
import { useTheme } from "@/lib/theme";

type Props = {
  value: string;
  language: string;
  height?: number;
  onChange: (v: string) => void;
  ariaLabel?: string;
};

/**
 * Lazy Monaco — `import('monaco-editor')` only when mounted so the main
 * admin chunk stays ~100 kB gzipped. Falls back to textarea when monaco
 * fails (e.g. CSP, offline) or during tests (jsdom has no layout).
 */
export function MonacoPane({ value, language, height = 240, onChange, ariaLabel }: Props) {
  const ref = React.useRef<HTMLDivElement>(null);
  const editorRef = React.useRef<unknown>(null);
  const [failed, setFailed] = React.useState(false);
  const { resolved } = useTheme();
  const monacoTheme = resolved === "dark" ? "vs-dark" : "vs";

  React.useEffect(() => {
    let disposed = false;
    let editor: { dispose: () => void; onDidChangeModelContent: (cb: () => void) => { dispose: () => void }; setValue: (v: string) => void; getValue: () => string } | null = null;

    (async () => {
      try {
        const monaco = await import("monaco-editor");
        if (disposed || !ref.current) return;
        // Workers would need extra config; disable workers for now.
        (self as unknown as Record<string, unknown>).MonacoEnvironment = { getWorker: () => ({ postMessage: () => {}, addEventListener: () => {} }) as unknown as Worker };
        editor = monaco.editor.create(ref.current, {
          value,
          language: language === "plain" ? "plaintext" : language,
          minimap: { enabled: false },
          lineNumbers: "on",
          scrollBeyondLastLine: false,
          fontSize: 13,
          wordWrap: "on",
          automaticLayout: true,
          theme: monacoTheme,
        }) as unknown as typeof editor;
        editorRef.current = editor;
        editor!.onDidChangeModelContent(() => onChange(editor!.getValue()));
      } catch {
        if (!disposed) setFailed(true);
      }
    })();

    return () => {
      disposed = true;
      if (editor) editor.dispose();
    };
  }, []); // mount once; value/language updates handled below

  React.useEffect(() => {
    const ed = editorRef.current as { setValue: (v: string) => void; getValue: () => string } | null;
    if (ed && ed.getValue() !== value) ed.setValue(value);
  }, [value]);

  // The pane follows the admin's light/dark choice, live.
  React.useEffect(() => {
    if (!editorRef.current) return;
    void import("monaco-editor").then((m) => m.editor.setTheme(monacoTheme)).catch(() => undefined);
  }, [monacoTheme]);

  if (failed) {
    return (
      <textarea
        aria-label={ariaLabel}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        rows={Math.ceil(height / 20)}
        className="w-full rounded-md border bg-background p-2 font-mono text-xs"
      />
    );
  }

  return <div ref={ref} style={{ height }} className="overflow-hidden rounded-md border" aria-label={ariaLabel} />;
}
