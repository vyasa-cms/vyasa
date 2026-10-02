import type { ReactNode } from "react";

export const metadata = {
  title: "Vyasa headless example",
  description: "A Next.js frontend reading from a Vyasa instance.",
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body
        style={{
          fontFamily: "system-ui, sans-serif",
          maxWidth: "42rem",
          margin: "0 auto",
          padding: "2rem 1rem",
          lineHeight: 1.6,
        }}
      >
        {children}
      </body>
    </html>
  );
}
