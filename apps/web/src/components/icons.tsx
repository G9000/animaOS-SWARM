import type { SVGProps } from 'react';

type IconProps = SVGProps<SVGSVGElement> & { size?: number };

function base({ size = 16, ...props }: IconProps, path: React.ReactNode) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      {...props}
    >
      {path}
    </svg>
  );
}

export const SendIcon = (p: IconProps) =>
  base(p, <path d="M12 19V5M5 12l7-7 7 7" />);
export const GearIcon = (p: IconProps) =>
  base(
    p,
    <>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </>,
  );
export const XIcon = (p: IconProps) =>
  base(p, <path d="M18 6 6 18M6 6l12 12" />);
export const TrashIcon = (p: IconProps) =>
  base(
    p,
    <path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2m3 0v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" />,
  );
export const SparkIcon = (p: IconProps) =>
  base(
    p,
    <path d="M12 3v3m0 12v3M5.6 5.6l2.2 2.2m8.4 8.4 2.2 2.2M3 12h3m12 0h3M5.6 18.4l2.2-2.2m8.4-8.4 2.2-2.2" />,
  );
export const PulseIcon = (p: IconProps) =>
  base(p, <path d="M22 12h-4l-3 9L9 3l-3 9H2" />);
export const BoltIcon = (p: IconProps) =>
  base(p, <path d="M13 2 3 14h7l-1 8 10-12h-7l1-8z" />);
export const ChevronIcon = (p: IconProps) => base(p, <path d="m6 9 6 6 6-6" />);
export const AlertIcon = (p: IconProps) =>
  base(
    p,
    <>
      <path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
      <path d="M12 9v4M12 17h.01" />
    </>,
  );
export const ChipIcon = (p: IconProps) =>
  base(
    p,
    <>
      <rect x="6" y="6" width="12" height="12" rx="2" />
      <path d="M9 2v2m6-2v2M9 20v2m6-2v2M2 9h2m-2 6h2m16-6h2m-2 6h2" />
    </>,
  );
export const PlusIcon = (p: IconProps) =>
  base(p, <path d="M12 5v14M5 12h14" />);
export const ShieldIcon = (p: IconProps) =>
  base(p, <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />);
export const AgentsIcon = (p: IconProps) =>
  base(
    p,
    <>
      <path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" />
      <circle cx="9" cy="7" r="4" />
      <path d="M22 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75" />
    </>,
  );
