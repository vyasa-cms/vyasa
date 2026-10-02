import Link from "next/link";
import { listPosts } from "@/lib/vyasa";

export default async function HomePage() {
  const posts = await listPosts(20);

  return (
    <main>
      <h1>Latest posts</h1>
      {posts.length === 0 ? (
        <p>
          No published posts yet. Seed some with{" "}
          <code>vyasa dev seed --count 20</code>.
        </p>
      ) : (
        <ul style={{ listStyle: "none", padding: 0 }}>
          {posts.map((post) => (
            // Keyed on slug: the numeric id loses precision in JavaScript.
            <li key={post.slug} style={{ marginBottom: "1.5rem" }}>
              <h2 style={{ marginBottom: "0.25rem" }}>
                <Link href={`/posts/${post.slug}`}>{post.title}</Link>
              </h2>
              {post.publishedAt ? (
                <time
                  dateTime={post.publishedAt}
                  style={{ fontSize: "0.875rem", opacity: 0.7 }}
                >
                  {new Date(post.publishedAt).toLocaleDateString()}
                </time>
              ) : null}
              {post.excerpt ? <p>{post.excerpt}</p> : null}
            </li>
          ))}
        </ul>
      )}
    </main>
  );
}
