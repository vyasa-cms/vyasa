import * as React from "react";
import { NodeViewContent, NodeViewWrapper, type ReactNodeViewProps } from "@tiptap/react";
import { ExternalLink, ImagePlus, Link2, Replace, Sparkles, Trash2, Type } from "lucide-react";
import { api } from "@/api/client";
import { notify } from "@/components/ui/toast";
import { MediaPicker } from "../MediaPicker";
import { mediaUrl } from "../blocks";
import { replaceImage } from "./upload";
import { useAiAvailable } from "../useAiAvailable";
import { cn } from "@/lib/utils";

type Tool = "alt" | "library" | "link" | "generate" | null;

/**
 * An image rendered as an image. The caption is ordinary editable text
 * underneath; alt text, replacing and removing live in a small toolbar that
 * appears when the figure is selected. With no file yet it is an inviting
 * empty state rather than a URL field.
 */
export function ImageView({
  node,
  editor,
  getPos,
  selected,
  updateAttributes,
  deleteNode,
}: ReactNodeViewProps) {
  const url = typeof node.attrs["url"] === "string" ? node.attrs["url"] : "";
  const alt = typeof node.attrs["alt"] === "string" ? node.attrs["alt"] : "";
  const pending = typeof node.attrs["pending"] === "string" ? node.attrs["pending"] : null;
  const src = url !== "" ? url : pending;

  const [tool, setTool] = React.useState<Tool>(null);
  const [linkDraft, setLinkDraft] = React.useState("");
  const [prompt, setPrompt] = React.useState("");
  const [generating, setGenerating] = React.useState(false);
  const ai = useAiAvailable();

  const generate = async () => {
    if (prompt.trim() === "" || generating) return;
    setGenerating(true);
    try {
      const media = await api.generateImage(prompt.trim());
      updateAttributes({ url: mediaUrl(media.id), pending: null, alt: alt !== "" ? alt : (media.alt ?? "") });
      setTool(null);
      notify.success("Image generated", "It's in your media library too.");
    } catch (e) {
      notify.error("Couldn't generate the image", e);
    } finally {
      setGenerating(false);
    }
  };
  const fileRef = React.useRef<HTMLInputElement>(null);

  React.useEffect(() => {
    if (!selected) setTool(null);
  }, [selected]);

  const position = (): number | null => {
    const pos = getPos();
    return typeof pos === "number" ? pos : null;
  };

  const onFile = (file: File | undefined) => {
    const pos = position();
    if (file === undefined || pos === null) return;
    replaceImage(editor, pos, file);
    setTool(null);
  };

  const applyLink = () => {
    const value = linkDraft.trim();
    if (value === "") return;
    updateAttributes({
      url: /^(https?:)?\/\//.test(value) || value.startsWith("/") ? value : `https://${value}`,
      pending: null,
    });
    setLinkDraft("");
    setTool(null);
  };

  const keep = (e: React.SyntheticEvent) => e.preventDefault(); // keep the node selected

  return (
    <NodeViewWrapper
      as="figure"
      data-vy-image=""
      className={cn("vy-image", selected && "is-selected", src === null && "is-empty-image")}
    >
      {src !== null ? (
        <div className="relative" contentEditable={false}>
          <img src={src} alt={alt} draggable={false} loading="lazy" />
          {pending !== null && url === "" ? (
            <div className="absolute inset-0 grid place-items-center bg-background/60 text-xs font-medium backdrop-blur-[1px]">
              Uploading…
            </div>
          ) : null}

          {selected ? (
            <div className="vy-image-tools" role="toolbar" aria-label="Image tools">
              <ToolButton
                label={alt === "" ? "Add alt text" : "Alt text"}
                emphasis={alt === ""}
                onMouseDown={keep}
                onClick={() => setTool(tool === "alt" ? null : "alt")}
              >
                <Type className="h-3.5 w-3.5" aria-hidden="true" />
                {alt === "" ? "Add alt text" : "Alt"}
              </ToolButton>
              <ToolButton label="Replace with an upload" onMouseDown={keep} onClick={() => fileRef.current?.click()}>
                <Replace className="h-3.5 w-3.5" aria-hidden="true" />
                Replace
              </ToolButton>
              <ToolButton label="Choose from the library" onMouseDown={keep} onClick={() => setTool(tool === "library" ? null : "library")}>
                <ImagePlus className="h-3.5 w-3.5" aria-hidden="true" />
                Library
              </ToolButton>
              <ToolButton label="Use an image address" onMouseDown={keep} onClick={() => { setLinkDraft(url); setTool(tool === "link" ? null : "link"); }}>
                <Link2 className="h-3.5 w-3.5" aria-hidden="true" />
              </ToolButton>
              {url !== "" ? (
                <a
                  href={url}
                  target="_blank"
                  rel="noopener noreferrer"
                  aria-label="Open the image in a new tab"
                  className="inline-flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
                >
                  <ExternalLink className="h-3.5 w-3.5" aria-hidden="true" />
                </a>
              ) : null}
              <ToolButton label="Remove image" destructive onMouseDown={keep} onClick={() => deleteNode()}>
                <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
              </ToolButton>
            </div>
          ) : null}
        </div>
      ) : (
        <div
          contentEditable={false}
          className="vy-image-empty"
          data-testid="image-empty-state"
        >
          <p className="text-sm font-medium">Add an image</p>
          <div className="mt-2 flex flex-wrap justify-center gap-2">
            <ToolButton label="Upload an image" emphasis onClick={() => fileRef.current?.click()}>
              <ImagePlus className="h-3.5 w-3.5" aria-hidden="true" />
              Upload
            </ToolButton>
            <ToolButton label="Choose from the library" onClick={() => setTool(tool === "library" ? null : "library")}>
              Library
            </ToolButton>
            <ToolButton label="Use an image address" onClick={() => setTool(tool === "link" ? null : "link")}>
              <Link2 className="h-3.5 w-3.5" aria-hidden="true" />
              Address
            </ToolButton>
            {ai.image ? (
              <ToolButton label="Generate an image from a description" onClick={() => setTool(tool === "generate" ? null : "generate")}>
                <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
                Generate
              </ToolButton>
            ) : null}
          </div>
          <p className="mt-2 text-xs text-muted-foreground">
            You can also drop a file here or paste one. Not saved until it has an image.
          </p>
        </div>
      )}

      {tool === "alt" ? (
        <div contentEditable={false} className="vy-image-panel">
          <label className="block text-xs font-medium" htmlFor={`alt-${node.attrs["uploadId"] ?? "img"}`}>
            Alt text
          </label>
          <input
            id={`alt-${node.attrs["uploadId"] ?? "img"}`}
            autoFocus
            value={alt}
            onChange={(e) => updateAttributes({ alt: e.target.value })}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === "Escape") {
                e.preventDefault();
                setTool(null);
                editor.commands.focus();
              }
            }}
            placeholder="Describe the image for people who can't see it"
            className="mt-1 h-8 w-full rounded-md border border-input bg-background px-2 text-sm focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
          />
          <p className="mt-1 text-[11px] text-muted-foreground">
            Leave it empty only if the image is purely decorative.
          </p>
        </div>
      ) : null}

      {tool === "library" ? (
        <div contentEditable={false} className="vy-image-panel">
          <MediaPicker
            accept="image"
            onPick={(m) => {
              updateAttributes({ url: mediaUrl(m.id), pending: null, alt: alt !== "" ? alt : (m.alt ?? "") });
              setTool(null);
            }}
          />
        </div>
      ) : null}

      {tool === "generate" ? (
        <form
          contentEditable={false}
          className="vy-image-panel space-y-2"
          data-testid="image-generate"
          onSubmit={(e) => {
            e.preventDefault();
            void generate();
          }}
        >
          <label className="block text-xs font-medium" htmlFor={`gen-${node.attrs["uploadId"] ?? "img"}`}>
            Describe the image
          </label>
          <textarea
            id={`gen-${node.attrs["uploadId"] ?? "img"}`}
            autoFocus
            rows={2}
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            placeholder="A watercolour of a lighthouse at dusk, soft light, no text"
            className="w-full rounded-md border border-input bg-background p-2 text-sm focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
          />
          <div className="flex items-center gap-2">
            <button type="submit" disabled={generating || prompt.trim() === ""} className="h-8 rounded-md bg-primary px-2.5 text-xs font-medium text-primary-foreground disabled:opacity-60">
              {generating ? "Generating…" : "Generate"}
            </button>
            <span className="text-[11px] text-muted-foreground">Uses the image model on the Models page; counts against the daily cap.</span>
          </div>
        </form>
      ) : null}

      {tool === "link" ? (
        <form
          contentEditable={false}
          className="vy-image-panel flex items-center gap-1.5"
          onSubmit={(e) => {
            e.preventDefault();
            applyLink();
          }}
        >
          <input
            autoFocus
            value={linkDraft}
            onChange={(e) => setLinkDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.preventDefault();
                setTool(null);
              }
            }}
            placeholder="https://… or /api/v1/media/…/raw"
            aria-label="Image address"
            className="h-8 flex-1 rounded-md border border-input bg-background px-2 font-mono text-xs focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
          />
          <button type="submit" className="h-8 rounded-md bg-primary px-2.5 text-xs font-medium text-primary-foreground">
            Use
          </button>
        </form>
      ) : null}

      <NodeViewContent<"figcaption"> as="figcaption" className="vy-image-caption" />

      <input
        ref={fileRef}
        type="file"
        accept="image/*,.heic,.heif"
        hidden
        onChange={(e) => {
          onFile(e.target.files?.[0]);
          e.target.value = "";
        }}
      />
    </NodeViewWrapper>
  );
}

function ToolButton({
  label,
  emphasis,
  destructive,
  onClick,
  onMouseDown,
  children,
}: {
  label: string;
  emphasis?: boolean;
  destructive?: boolean;
  onClick: () => void;
  onMouseDown?: (e: React.SyntheticEvent) => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onMouseDown={onMouseDown}
      onClick={onClick}
      className={cn(
        "inline-flex h-7 items-center gap-1 rounded-md px-2 text-xs font-medium transition-colors",
        emphasis === true
          ? "bg-primary text-primary-foreground hover:bg-primary/90"
          : destructive === true
            ? "text-muted-foreground hover:bg-destructive-subtle hover:text-destructive"
            : "text-muted-foreground hover:bg-accent hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}
