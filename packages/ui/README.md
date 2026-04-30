# @animaOS-SWARM/ui

Tailwind-only UI primitives for AnimaOS.

## Usage

```tsx
import { Button, Box, Text } from '@animaOS-SWARM/ui';
```

## Primitives

- **Button** – `variant` (`default` | `ghost` | `outline`), `size` (`sm` | `md` | `lg`)
- **Box** – unstyled layout container
- **Text** – `as`, `size`, `weight`, `color`

All components are styled with Tailwind CSS utility classes. The consuming app must import Tailwind CSS and reference this package via `@source` so Tailwind scans the component source files.
