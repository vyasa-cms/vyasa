import * as React from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "@/api/client";
import { Button } from "@/components/ui/button";
import { notify } from "@/components/ui/toast";

/**
 * Take the site's content out, or bring an archive in. Export is a plain
 * link so the browser saves the file; import reads a JSON archive and
 * reports what it created. Running an import twice adds nothing.
 */
export function ExportImportPanel() {
  const queryClient = useQueryClient();
  const [file, setFile] = React.useState<File | null>(null);
  const [withOptions, setWithOptions] = React.useState(false);
  const [report, setReport] = React.useState<{ summary: string; warnings: string[] } | null>(null);
  const run = useMutation({
    mutationFn: async () => {
      if (!file) throw new Error("choose an archive first");
      const text = await file.text();
      let archive: unknown;
      try {
        archive = JSON.parse(text);
      } catch {
        throw new Error("that file is not a JSON archive");
      }
      return api.importArchive(archive, withOptions);
    },
    onSuccess: (r) => {
      const parts = [`${r.posts} posts`, `${r.terms} terms`, `${r.comments} comments`, `${r.users} users`, `${r.menus} menus`];
      if ((r.content_types ?? 0) > 0) parts.push(`${r.content_types} content types`);
      if ((r.content_fields ?? 0) > 0) parts.push(`${r.content_fields} fields`);
      if (r.options > 0) parts.push(`${r.options} options`);
      // Every warning, not a sample: each names something that did not come
      // across (a field value whose media or entry is not here, say).
      setReport({ summary: `Created ${parts.join(", ")}; ${r.skipped} already present.`, warnings: r.warnings });
      setFile(null);
      void queryClient.invalidateQueries();
      notify.success("Import finished", `${r.posts} posts created.`);
    },
    onError: (e) => notify.error("Import failed", e),
  });
  return (
    <div className="space-y-4" data-testid="export-import">
      <div>
        <h3 className="text-sm font-medium">Export</h3>
        <p className="mb-2 mt-0.5 max-w-[70ch] text-xs text-muted-foreground">
          Everything but media files and secrets: users, categories and tags, posts and pages with their blocks, comments, menus, and site options. The Vyasa archive imports back into any Vyasa site; the WordPress file goes into WordPress's own importer (Tools → Import).
        </p>
        <div className="flex flex-wrap gap-2">
          <a href="/api/v1/export?format=json" className="inline-flex h-8 items-center rounded-md border px-3 text-sm hover:bg-accent" download>Download Vyasa archive (.json)</a>
          <a href="/api/v1/export?format=wxr" className="inline-flex h-8 items-center rounded-md border px-3 text-sm hover:bg-accent" download>Download for WordPress (.xml)</a>
        </div>
      </div>
      <div>
        <h3 className="text-sm font-medium">Import a Vyasa archive</h3>
        <p className="mb-2 mt-0.5 max-w-[70ch] text-xs text-muted-foreground">
          Existing users, terms and posts are matched by email or slug and left alone, so importing the same archive twice adds nothing. Media files are not copied; blocks keep their URLs. Coming from WordPress? Use <code className="font-mono">vyasa import wp</code> on the server.
        </p>
        <div className="flex flex-wrap items-center gap-2">
          <input type="file" accept=".json,application/json" aria-label="archive file" onChange={(e) => { setFile(e.target.files?.[0] ?? null); setReport(null); }} className="text-sm file:mr-3 file:rounded-md file:border file:border-input file:bg-background file:px-3 file:py-1.5 file:text-sm" />
          <label className="inline-flex items-center gap-1.5 text-xs"><input type="checkbox" checked={withOptions} onChange={(e) => setWithOptions(e.target.checked)} className="accent-primary" />Also apply its site options</label>
          <Button size="sm" disabled={!file || run.isPending} onClick={() => run.mutate()}>{run.isPending ? "Importing…" : "Import"}</Button>
        </div>
        {report ? (
          <div className="mt-2 text-xs text-muted-foreground" data-testid="import-report">
            <p>{report.summary}</p>
            {report.warnings.length > 0 ? (
              <>
                <p className="mt-1 font-medium text-warning">{report.warnings.length === 1 ? "1 warning" : `${report.warnings.length} warnings`}:</p>
                <ul className="mt-0.5 max-h-48 list-disc space-y-0.5 overflow-y-auto pl-4">
                  {report.warnings.map((w, i) => <li key={i}>{w}</li>)}
                </ul>
              </>
            ) : null}
          </div>
        ) : null}
      </div>
    </div>
  );
}
