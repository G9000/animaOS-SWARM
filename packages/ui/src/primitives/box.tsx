import { type HTMLAttributes, forwardRef } from 'react';

export interface BoxProps extends HTMLAttributes<HTMLDivElement> {}

export const Box = forwardRef<HTMLDivElement, BoxProps>(
  ({ className = '', ...props }, ref) => {
    return <div ref={ref} className={className} {...props} />;
  }
);

Box.displayName = 'Box';
