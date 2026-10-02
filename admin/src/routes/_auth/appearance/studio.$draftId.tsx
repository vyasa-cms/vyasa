import { createFileRoute, useNavigate } from "@tanstack/react-router";

import { StudioPage } from "@/components/studio/StudioPage";

export const Route = createFileRoute("/_auth/appearance/studio/$draftId")({
  component: StudioRoute,
  // Three panes with fixed rails: capping the page at a reading width left
  // the preview small and the rest of a wide screen empty.
  staticData: { wide: true },
});

function StudioRoute() {
  const { draftId } = Route.useParams();
  const navigate = useNavigate();
  // No `?tab=assistant` any more: the assistant is a pane of the studio,
  // always on screen, so there is nothing to deep-link to.
  return (
    <StudioPage draftId={draftId} onExit={() => void navigate({ to: "/appearance" })} />
  );
}
