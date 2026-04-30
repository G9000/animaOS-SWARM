import { type HTMLAttributes, forwardRef } from 'react';

export interface TextProps extends HTMLAttributes<HTMLSpanElement> {
  as?: 'span' | 'p' | 'label' | 'small';
  size?: 'xs' | 'sm' | 'base' | 'lg' | 'xl';
  weight?: 'normal' | 'medium' | 'semibold' | 'bold';
  color?: 'primary' | 'secondary' | 'muted';
}

export const Text = forwardRef<HTMLSpanElement, TextProps>(
  (
    {
      as: Component = 'span',
      size = 'base',
      weight = 'normal',
      color = 'primary',
      className = '',
      ...props
    },
    ref
  ) => {
    const sizes: Record<NonNullable<TextProps['size']>, string> = {
      xs: 'text-xs',
      sm: 'text-sm',
      base: 'text-base',
      lg: 'text-lg',
      xl: 'text-xl',
    };

    const weights: Record<NonNullable<TextProps['weight']>, string> = {
      normal: 'font-normal',
      medium: 'font-medium',
      semibold: 'font-semibold',
      bold: 'font-bold',
    };

    const colors: Record<NonNullable<TextProps['color']>, string> = {
      primary: 'text-amber-100',
      secondary: 'text-amber-200/80',
      muted: 'text-amber-200/50',
    };

    const classes = [sizes[size], weights[weight], colors[color], className]
      .filter(Boolean)
      .join(' ');

    return <Component ref={ref as never} className={classes} {...props} />;
  }
);

Text.displayName = 'Text';
