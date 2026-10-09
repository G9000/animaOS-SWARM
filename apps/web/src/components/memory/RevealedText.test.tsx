import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { RevealedText } from './RevealedText';

describe('RevealedText', () => {
  it('reveals invisible characters in text', () => {
    const zwsp = String.fromCodePoint(0x200b);
    render(<RevealedText text={`a${zwsp}b`} />);
    expect(screen.getByText('a⟨U+200B⟩b')).toBeInTheDocument();
    expect(
      screen.getByText('This text contains 1 invisible character'),
    ).toBeInTheDocument();
  });

  it('renders plain text without a note', () => {
    const { container } = render(<RevealedText text="hello" className="x" />);
    expect(screen.getByText('hello')).toHaveClass('x');
    expect(container.querySelector('.memory-hidden-note')).toBeNull();
  });

  it('never renders markup', () => {
    const { container } = render(<RevealedText text="<b>x</b>" />);
    expect(screen.getByText('<b>x</b>')).toBeInTheDocument();
    expect(container.querySelector('b')).toBeNull();
  });
});
