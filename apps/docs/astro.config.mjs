import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import lucode from 'lucode-starlight';

export default defineConfig({
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
      plugins: [lucode({ footerText: 'Built with [animaOS](https://github.com/G9000/animaOS-SWARM).' })],
      sidebar: [
        { label: 'Overview', link: '/overview/' },
        {
          label: 'SDK',
          items: [
            { label: 'Quick Start', link: '/sdk/quickstart/' },
            { label: 'Agents', link: '/sdk/agents/' },
            { label: 'Swarms', link: '/sdk/swarms/' },
            { label: 'Memories', link: '/sdk/memories/' },
            { label: 'API Reference', link: '/sdk/reference/' },
          ],
        },
        {
          label: 'Tools',
          items: [
            { label: 'CLI (Command Line Interface)', link: '/cli/' },
            { label: 'TUI (Terminal User Interface)', link: '/tui/' },
          ],
        },

      ],
      customCss: ['./src/styles/custom.css'],
      favicon: '/favicon.png',
    }),
  ],
});
