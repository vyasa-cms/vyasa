import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  getPluginSettings,
  getPluginSurface,
  putPluginSetting,
  type PluginField,
  type PluginForm,
} from "@/api/plugins";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Chip, Panel } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";

/**
 * What installed plugins add to the site, and the settings forms they
 * declare.
 *
 * A plugin's admin surface is data, never markup: it declares fields and
 * this renders them. That is the whole difference between extending the
 * admin and letting a third party run code in it.
 */
export function PluginSurfacePanels() {
  const surface = useQuery({
    queryKey: ["plugin-surface"],
    queryFn: getPluginSurface,
  });

  if (surface.isPending || surface.error !== null) return null;
  const s = surface.data;
  const nothing =
    s.blocks.length === 0 &&
    s.routes.length === 0 &&
    s.postTypes.length === 0 &&
    s.taxonomies.length === 0 &&
    s.tasks.length === 0;

  return (
    <>
      {s.forms.map((form) => (
        <SettingsForm key={form.pluginId} form={form} />
      ))}

      {nothing ? null : (
        <Panel
          testId="plugin-surface"
          title="What your plugins add"
          description="Blocks, content types, endpoints and background work contributed by the plugins you have enabled."
        >
          <div className="space-y-4 text-sm">
            <Group title="Blocks">
              {s.blocks.map((b) => (
                <Chip key={b.kind} dot={false}>
                  {b.icon === null ? "" : `${b.icon} `}
                  {b.title}
                  <span className="ml-1 text-muted-foreground">{b.kind}</span>
                </Chip>
              ))}
            </Group>

            <Group title="Content types">
              {s.postTypes.map((t) => (
                <Chip key={t.slug} dot={false}>
                  {t.plural}
                  <span className="ml-1 text-muted-foreground">/{t.slug}</span>
                </Chip>
              ))}
            </Group>

            <Group title="Ways to file things">
              {s.taxonomies.map((t) => (
                <Chip key={t.slug} dot={false}>
                  {t.plural}
                  <span className="ml-1 text-muted-foreground">/{t.slug}</span>
                </Chip>
              ))}
            </Group>

            <Group title="Endpoints">
              {s.routes.map((r) => (
                <Chip key={`${r.method} ${r.path}`} dot={false}>
                  <code>
                    {r.method} {r.path}
                  </code>
                </Chip>
              ))}
            </Group>

            {s.tasks.length === 0 ? null : (
              <div>
                <h3 className="mb-1.5 text-xs font-medium uppercase tracking-wide text-muted-foreground">
                  Scheduled work
                </h3>
                <ul className="space-y-1" data-testid="plugin-tasks">
                  {s.tasks.map((t) => (
                    <li
                      key={`${t.pluginId}-${t.name}`}
                      className="flex flex-wrap items-center gap-2"
                    >
                      <Chip
                        tone={t.lastStatus === "failed" ? "warning" : "neutral"}
                      >
                        {t.name}
                      </Chip>
                      <span className="text-xs text-muted-foreground">
                        every {formatInterval(t.everySeconds)}
                        {t.lastRunAt === null
                          ? " · not run yet"
                          : ` · last ran ${new Date(t.lastRunAt).toLocaleString()}`}
                      </span>
                      {t.lastError === null ? null : (
                        <span className="text-xs text-destructive">
                          {t.lastError}
                        </span>
                      )}
                    </li>
                  ))}
                </ul>
              </div>
            )}
          </div>
        </Panel>
      )}
    </>
  );
}

function Group({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode[];
}) {
  if (children.length === 0) return null;
  return (
    <div>
      <h3 className="mb-1.5 text-xs font-medium uppercase tracking-wide text-muted-foreground">
        {title}
      </h3>
      <div className="flex flex-wrap gap-1.5">{children}</div>
    </div>
  );
}

/** Seconds as something readable at a glance. */
export function formatInterval(seconds: number): string {
  if (seconds % 3600 === 0) {
    const hours = seconds / 3600;
    return hours === 1 ? "hour" : `${hours} hours`;
  }
  if (seconds % 60 === 0) {
    const minutes = seconds / 60;
    return minutes === 1 ? "minute" : `${minutes} minutes`;
  }
  return `${seconds} seconds`;
}

/** One plugin's declared settings form. */
export function SettingsForm({ form }: { form: PluginForm }) {
  const queryClient = useQueryClient();
  const stored = useQuery({
    queryKey: ["plugin-settings", form.pluginId],
    queryFn: () => getPluginSettings(form.pluginId),
  });
  const [draft, setDraft] = React.useState<Record<string, unknown> | null>(
    null,
  );

  const values =
    draft ??
    Object.fromEntries(
      form.fields.map((f) => [
        f.key,
        stored.data?.[f.key] ?? f.default ?? defaultFor(f),
      ]),
    );

  const save = useMutation({
    mutationFn: async () => {
      // One request per field: the settings store is a key/value table and
      // there is no batch endpoint. A form has at most a handful of fields.
      for (const field of form.fields) {
        await putPluginSetting(form.pluginId, field.key, values[field.key]);
      }
    },
    onSuccess: () => {
      setDraft(null);
      void queryClient.invalidateQueries({
        queryKey: ["plugin-settings", form.pluginId],
      });
      // A settings save restarts the plugin under the enable rules, which
      // can change its status (or turn it off over a slug clash).
      void queryClient.invalidateQueries({ queryKey: ["plugins"] });
      void queryClient.invalidateQueries({ queryKey: ["plugin-surface"] });
      notify.success(`${form.pluginName} settings saved`);
    },
    onError: (e) => notify.error("Couldn't save the settings", e),
  });

  const set = (key: string, value: unknown) =>
    setDraft({ ...values, [key]: value });

  return (
    <Panel
      testId={`plugin-form-${form.pluginName}`}
      title={form.title}
      description={form.description ?? undefined}
    >
      <div className="space-y-3">
        {form.fields.map((field) => (
          <Field
            key={field.key}
            field={field}
            value={values[field.key]}
            onChange={(v) => set(field.key, v)}
          />
        ))}
        <Button
          disabled={draft === null || save.isPending}
          onClick={() => save.mutate()}
          data-testid={`plugin-form-save-${form.pluginName}`}
        >
          {save.isPending ? "Saving…" : "Save settings"}
        </Button>
      </div>
    </Panel>
  );
}

function defaultFor(field: PluginField): unknown {
  if (field.kind === "boolean") return false;
  if (field.kind === "number") return 0;
  if (field.kind === "select") return field.options[0] ?? "";
  return "";
}

function Field({
  field,
  value,
  onChange,
}: {
  field: PluginField;
  value: unknown;
  onChange: (value: unknown) => void;
}) {
  const label = <span className="text-sm font-medium">{field.label}</span>;
  const help =
    field.help === null ? null : (
      <span className="text-xs text-muted-foreground">{field.help}</span>
    );

  if (field.kind === "boolean") {
    return (
      <label className="flex items-start gap-2.5">
        <input
          type="checkbox"
          checked={value === true}
          aria-label={field.label}
          onChange={(e) => onChange(e.target.checked)}
          className="mt-0.5 h-3.5 w-3.5 shrink-0 accent-primary"
        />
        <span className="flex flex-col gap-0.5">
          {label}
          {help}
        </span>
      </label>
    );
  }

  return (
    <label className="flex flex-col gap-1.5">
      {label}
      {field.kind === "select" ? (
        <select
          aria-label={field.label}
          value={typeof value === "string" ? value : ""}
          onChange={(e) => onChange(e.target.value)}
          className="h-9 rounded-md border border-input bg-background px-3 text-sm"
        >
          {field.options.map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </select>
      ) : field.kind === "textarea" ? (
        <textarea
          aria-label={field.label}
          value={typeof value === "string" ? value : ""}
          onChange={(e) => onChange(e.target.value)}
          rows={4}
          className="rounded-md border border-input bg-background px-3 py-2 text-sm"
        />
      ) : (
        <Input
          type={field.kind === "number" ? "number" : "text"}
          aria-label={field.label}
          value={
            typeof value === "string" || typeof value === "number"
              ? String(value)
              : ""
          }
          onChange={(e) =>
            onChange(
              field.kind === "number" ? Number(e.target.value) : e.target.value,
            )
          }
        />
      )}
      {help}
    </label>
  );
}
