import Link from "next/link";
import { notFound } from "next/navigation";
import { getPostBySlug, renderBlocks } from "@/lib/vyasa";

export default async function PostPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  const post = await getPostBySlug(slug);
  if (!post) notFound();

  return (
    <main>
      <p>
        <Link href="/">← All posts</Link>
      </p>
      <h1>{post.title}</h1>
      {post.publishedAt ? (
        <time
          dateTime={post.publishedAt}
          style={{ fontSize: "0.875rem", opacity: 0.7 }}
        >
          {new Date(post.publishedAt).toLocaleDateString()}
        </time>
      ) : null}
      {/*
        The block content comes from Vyasa already validated against its
        schema, and renderBlocks escapes every text value it emits, so this
        is not a route for author-supplied markup.
      */}
      <article dangerouslySetInnerHTML={{ __html: renderBlocks(post.content) }} />
    </main>
  );
}
