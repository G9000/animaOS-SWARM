import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import lucode from 'lucode-starlight';

const site = process.env.ANIMAOS_DOCS_SITE ?? 'https://g9000.github.io/animaOS-SWARM';

function docsSidebarOverride() {
  return {
    name: 'docs-sidebar-override',
    hooks: {
      'config:setup'({ config, updateConfig }) {
        updateConfig({
          components: {
            ...(config.components ?? {}),
            Hero: './src/components/Hero.astro',
            Sidebar: './src/components/Sidebar.astro',
            PageTitle: './src/components/PageTitle.astro',
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
  integrations: [
    starlight({
      title: 'animaOS',
      description: 'Agent runtime SDK and API documentation',
      logo: {
        src: './src/assets/logo.png',
        replacesTitle: false,
      },
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/G9000/animaOS-SWARM' },
      ],
      plugins: [
        lucode({ footerText: 'Built with [animaOS](https://github.com/G9000/animaOS-SWARM).' }),
        docsSidebarOverride(),
      ],
      sidebar: [
        { label: 'Overview', link: '/overview/' },
        {
          label: 'SDK',
          collapsed: false,
          items: [
            { label: 'Quick Start', link: '/sdk/quickstart/' },
            { label: 'Client Basics', link: '/sdk/client/' },
            {
              label: 'Agents',
              collapsed: false,
              items: [
                { label: 'Overview', link: '/sdk/agents/' },
                { label: 'Lifecycle & Retry', link: '/sdk/agents/lifecycle/' },
                { label: 'Actions & Plugins', link: '/sdk/agents/tools/' },
              ],
            },
            {
              label: 'Swarms',
              collapsed: false,
              items: [
                { label: 'Overview', link: '/sdk/swarms/' },
                { label: 'Event Streaming', link: '/sdk/swarms/events/' },
              ],
            },
            {
              label: 'Memories',
              collapsed: false,
              items: [
                { label: 'Overview', link: '/sdk/memories/' },
                { label: 'Search & Recall', link: '/sdk/memories/recall/' },
                { label: 'Evaluation & Retention', link: '/sdk/memories/operations/' },
              ],
            },
            { label: 'API Reference', link: '/sdk/reference/' },
          ],
        },
        {
          label: 'CLI',
          collapsed: false,
          items: [
            { label: 'Overview', link: '/cli/' },
            { label: 'Agency Workflow', link: '/cli/workspaces/' },
            { label: 'Daemon Commands', link: '/cli/daemon/' },
            { label: 'Providers & Environment', link: '/cli/providers/' },
          ],
        },
        {
          label: 'TUI',
          collapsed: false,
          items: [
            { label: 'Overview', link: '/tui/' },
            { label: 'Commands & States', link: '/tui/commands/' },
            { label: 'History & Resume', link: '/tui/history/' },
          ],
        },

      ],
      customCss: ['./src/styles/custom.css'],
      favicon: '/favicon.png',
    }),
  ],
});
