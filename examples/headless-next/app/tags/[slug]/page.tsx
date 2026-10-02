import Link from "next/link";
import { listPostsByTerm } from "@/lib/vyasa";

export default async function TagPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  const posts = await listPostsByTerm(slug);

  return (
    <main>
      <p>
        <Link href="/">← All posts</Link>
      </p>
      <h1>Tagged “{slug}”</h1>
      {posts.length === 0 ? (
        <p>Nothing published under this tag.</p>
      ) : (
        <ul style={{ listStyle: "none", padding: 0 }}>
          {posts.map((post) => (
            <li key={post.slug} style={{ marginBottom: "1.5rem" }}>
              <h2 style={{ marginBottom: "0.25rem" }}>
                <Link href={`/posts/${post.slug}`}>{post.title}</Link>
              </h2>
              {post.excerpt ? <p>{post.excerpt}</p> : null}
            </li>
          ))}
        </ul>
      )}
    </main>
  );
}
