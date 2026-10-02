import { useQuery } from "@tanstack/react-query";
import { api } from "@/api/client";

/** The shared account when this site is a public demo; `null` otherwise. */
export function useDemo(): { email: string; password: string } | null {
  const status = useQuery({
    queryKey: ["setup-status"],
    queryFn: () => api.setupStatus(),
    retry: false,
    staleTime: 5 * 60_000,
  });
  return status.data?.demo ?? null;
}

/** A slim strip across the top of the admin on a public demo. */
export function DemoBanner() {
  const demo = useDemo();
  if (demo === null) return null;
  return (
    <div
      role="note"
      className="border-b bg-warning-subtle px-4 py-1.5 text-center text-xs text-warning-foreground"
    >
      This is the Vyasa demo: anyone can sign in, and it resets every hour. Installing
      plugins, sending mail and anything that reaches other servers is switched off.
    </div>
  );
}
