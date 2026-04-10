import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { App } from './app';

describe('ui app boundary', () => {
  it('renders the operator dashboard shell', () => {
    const markup = renderToStaticMarkup(createElement(App));

    expect(markup).toContain('ANIMAOS CONTROL GRID');
    expect(markup).toContain('Agents');
    expect(markup).toContain('Swarms');
    expect(markup).toContain('Thin-browser operations deck for animaOS Kit');
  });
});
