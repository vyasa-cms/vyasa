import { createFileRoute } from "@tanstack/react-router";
import { FieldEditor } from "@/components/content/FieldEditor";

/** The fields of one content type: built-in, a plugin's or an administrator's. */
export const Route = createFileRoute("/_auth/content-types/$slug")({
  component: function TypeFields() {
    const { slug } = Route.useParams();
    return <FieldEditor slug={slug} />;
  },
});
