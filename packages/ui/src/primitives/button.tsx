import { type ButtonHTMLAttributes, forwardRef } from 'react';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'default' | 'ghost' | 'outline';
  size?: 'sm' | 'md' | 'lg';
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ variant = 'default', size = 'md', className = '', ...props }, ref) => {
    const base =
      'inline-flex items-center justify-center font-medium transition-colors focus:outline-none focus:ring-2 focus:ring-offset-2 disabled:pointer-events-none disabled:opacity-50';

    const variants: Record<NonNullable<ButtonProps['variant']>, string> = {
      default:
        'bg-amber-500 text-black hover:bg-amber-400 focus:ring-amber-500',
      ghost:
        'bg-transparent text-amber-200 hover:bg-amber-500/10 focus:ring-amber-500',
      outline:
        'border border-amber-500/30 bg-transparent text-amber-200 hover:bg-amber-500/10 focus:ring-amber-500',
    };

    const sizes: Record<NonNullable<ButtonProps['size']>, string> = {
      sm: 'h-8 px-3 text-sm rounded',
      md: 'h-10 px-4 text-base rounded-md',
      lg: 'h-12 px-6 text-lg rounded-lg',
    };

    const classes = [base, variants[variant], sizes[size], className]
      .filter(Boolean)
      .join(' ');

    return <button ref={ref} className={classes} {...props} />;
  }
);

Button.displayName = 'Button';
