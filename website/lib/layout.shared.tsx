import type { BaseLayoutProps } from 'fumadocs-ui/layouts/shared';
import { appName, gitConfig } from './shared';

export function baseOptions(): BaseLayoutProps {
  return {
    nav: {
      title: (
        <>
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img src="/mark.svg" alt="" width={22} height={22} />
          <span className="font-semibold">{appName}</span>
        </>
      ),
    },
    links: [
      { text: 'Docs', url: '/docs' },
      { text: 'Live demo', url: 'https://demo.vyasa.site', external: true },
      { text: 'Marketplace', url: 'https://github.com/vyasa-cms/marketplace', external: true },
    ],
    githubUrl: `https://github.com/${gitConfig.user}/${gitConfig.repo}`,
  };
}
