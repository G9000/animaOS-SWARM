import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import lucode from 'lucode-starlight';

const site = process.env.ANIMAOS_CONTRIBUTOR_DOCS_SITE ?? 'https://g9000.github.io/animaOS-SWARM/contributor-docs';

function contributorSidebarOverride() {
  return {
    name: 'contributor-sidebar-override',
    hooks: {
      'config:setup'({ config, updateConfig }) {
        updateConfig({
          components: {
            ...(config.components ?? {}),
            Sidebar: './src/components/Sidebar.astro',
          },
        });
      },
    },
  };
}

export default defineConfig({
  site,
  srcDir: './src',
  outDir: './dist',
  server: {
    port: 4322,
  },
  integrations: [
    starlight({
      title: 'animaOS Contributors',
      description: 'Contributor documentation for the animaOS monorepo',
      logo: { src: './src/assets/logo.png', replacesTitle: false },
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/G9000/animaOS-SWARM' },
      ],
      plugins: [
        lucode({ footerText: 'Built with [animaOS](https://github.com/G9000/animaOS-SWARM).' }),
        contributorSidebarOverride(),
      ],
      sidebar: [
        { label: 'Overview', link: '/overview/' },
        { label: 'Philosophy', link: '/philosophy/' },
        { label: 'Architecture', collapsed: false, items: [
          { label: 'Monorepo Structure', link: '/architecture/monorepo/' },
          { label: 'TypeScript Core', link: '/architecture/typescript-core/' },
        ]},
        { label: 'Rust Core', collapsed: false, items: [
          { label: 'Why Rust?', link: '/architecture/why-rust/' },
          { label: 'Overview', link: '/architecture/rust-core/' },
          { label: 'anima-core', collapsed: false, items: [
            { label: 'Overview', link: '/architecture/rust-core/anima-core/' },
            { label: 'AgentRuntime', link: '/architecture/rust-core/anima-core/agent-runtime/' },
            { label: 'Traits', link: '/architecture/rust-core/anima-core/traits/' },
            { label: 'Events', link: '/architecture/rust-core/anima-core/events/' },
          ]},
          { label: 'anima-memory', collapsed: false, items: [
            { label: 'Overview', link: '/architecture/rust-core/anima-memory/' },
            { label: 'Search Pipeline', link: '/architecture/rust-core/anima-memory/search/' },
            { label: 'Recall Fusion', link: '/architecture/rust-core/anima-memory/recall/' },
            { label: 'Evaluation', link: '/architecture/rust-core/anima-memory/evaluation/' },
          ]},
          { label: 'anima-swarm', collapsed: false, items: [
            { label: 'Overview', link: '/architecture/rust-core/anima-swarm/' },
            { label: 'Strategies', link: '/architecture/rust-core/anima-swarm/strategies/' },
            { label: 'Message Bus', link: '/architecture/rust-core/anima-swarm/message-bus/' },
          ]},
          { label: 'Building & Extending', link: '/architecture/rust-core/building/' },
        ]},
        { label: 'Rust Daemon', collapsed: false, items: [
          { label: 'Overview', link: '/hosts/rust-daemon/' },
          { label: 'Environment', link: '/hosts/rust-daemon/env/' },
          { label: 'HTTP API', link: '/hosts/rust-daemon/api/' },
          { label: 'Operations', link: '/hosts/rust-daemon/ops/' },
        ]},
        { label: 'Workflow', collapsed: false, items: [
          { label: 'Development', link: '/workflow/dev/' },
          { label: 'Testing', link: '/workflow/testing/' },
        ]},
      ],
      customCss: ['./src/styles/custom.css'],
      favicon: '/favicon.png',
    }),
  ],
});
