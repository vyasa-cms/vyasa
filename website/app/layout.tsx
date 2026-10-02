import { Inter } from 'next/font/google';
import { Provider } from '@/components/provider';
import './global.css';
import type { Metadata } from 'next';

export const metadata: Metadata = {
  title: { default: 'Vyasa — the AI-native CMS', template: '%s · Vyasa' },
  description: 'An AI-native content management system: a fast WordPress alternative written in Rust.',
  metadataBase: new URL('https://vyasa.site'),
};

const inter = Inter({
  subsets: ['latin'],
});

export default function Layout({ children }: LayoutProps<'/'>) {
  return (
    <html lang="en" className={inter.className} suppressHydrationWarning>
      <body className="flex flex-col min-h-screen">
        <Provider>{children}</Provider>
      </body>
    </html>
  );
}
