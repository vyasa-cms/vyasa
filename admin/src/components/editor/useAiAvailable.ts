import { useQuery } from "@tanstack/react-query";
import { api } from "@/api/client";

export interface AiAvailable {
  text: boolean;
  vision: boolean;
  image: boolean;
  transcription: boolean;
  speech: boolean;
  /** False until the first answer; every feature reads as available meanwhile. */
  loaded: boolean;
}

const NONE: AiAvailable = { text: true, vision: true, image: true, transcription: true, speech: true, loaded: false };

/**
 * Which AI affordances the editor should show. A button that can only
 * fail with "no text model is set up" is worse than no button, so each
 * surface asks here first. Cached for a minute; a request that fails
 * (an old server) leaves everything shown, as before.
 */
export function useAiAvailable(): AiAvailable {
  const q = useQuery({
    queryKey: ["ai-available"],
    queryFn: () => api.aiAvailable(),
    staleTime: 60_000,
    retry: false,
  });
  if (q.data === undefined) return NONE;
  return { ...q.data, loaded: true };
}
