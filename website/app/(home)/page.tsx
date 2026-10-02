import Link from 'next/link';

const features = [
  {
    title: 'AI built in',
    body: 'Writing help in the editor, an AI theme studio, semantic search and related posts, and an MCP server so agents work under the same permissions as people. Bring your own Anthropic or OpenAI-compatible provider.',
  },
  {
    title: 'Safe by construction',
    body: 'Themes are data, never code. Plugins are WebAssembly components that can only do what they declare, checked on every call. Every API route declares who may call it.',
  },
  {
    title: 'Your content model',
    body: 'A block editor, custom content types with typed fields, custom roles, REST and GraphQL APIs — in one Rust binary with PostgreSQL.',
  },
];

export default function HomePage() {
  return (
    <main className="mx-auto flex w-full max-w-5xl flex-1 flex-col gap-16 px-6 py-16">
      <section className="flex flex-col items-center gap-6 text-center">
        <h1 className="text-4xl font-bold tracking-tight sm:text-5xl">
          The AI-native content management system
        </h1>
        <p className="max-w-2xl text-lg text-fd-muted-foreground">
          Vyasa is a fast WordPress alternative written in Rust: structured content, themes as data,
          sandboxed plugins, and AI where it helps.
        </p>
        <div className="flex flex-wrap justify-center gap-3">
          <Link href="/docs/getting-started" className="rounded-md bg-fd-primary px-5 py-2.5 font-medium text-fd-primary-foreground">
            Get started
          </Link>
          <a href="https://demo.vyasa.site" className="rounded-md border px-5 py-2.5 font-medium">
            Try the demo
          </a>
          <a href="https://github.com/vyasa-cms/vyasa" className="rounded-md border px-5 py-2.5 font-medium">
            GitHub
          </a>
        </div>
      </section>

      <section className="grid gap-6 sm:grid-cols-3">
        {features.map((f) => (
          <div key={f.title} className="rounded-lg border p-5">
            <h2 className="mb-2 font-semibold">{f.title}</h2>
            <p className="text-sm text-fd-muted-foreground">{f.body}</p>
          </div>
        ))}
      </section>

      <section className="flex flex-col gap-3">
        <h2 className="text-xl font-semibold">Run it in a minute</h2>
        <pre tabIndex={0} className="overflow-x-auto rounded-lg border bg-fd-card p-4 text-sm">
{`mkdir vyasa && cd vyasa
curl -fsSLO https://raw.githubusercontent.com/vyasa-cms/vyasa/main/docker-compose.yml
docker compose run --rm app migrate
docker compose up -d
docker compose exec app cat .run/setup-token`}
        </pre>
        <p className="text-sm text-fd-muted-foreground">
          Then open <code>http://localhost:3000/admin/setup</code> and enter the token. Until 0.1.0 is
          released, set <code>VYASA_VERSION=0.1.0-rc.2</code> first. Full steps in{' '}
          <Link href="/docs/getting-started" className="underline">Getting started</Link>.
        </p>
      </section>
    </main>
  );
}
