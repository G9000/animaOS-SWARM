import React from 'react';
import { EventEmitter } from 'node:events';
import { render as inkRender } from 'ink';

class Stdout extends EventEmitter {
  get columns(): number {
    return 100;
  }

  frames: string[] = [];
  private lastRenderedFrame = '';

  write = (frame: string) => {
    this.frames.push(frame);
    this.lastRenderedFrame = frame;
    return true;
  };

  lastFrame = () => this.lastRenderedFrame;
}

class Stderr extends EventEmitter {
  frames: string[] = [];
  private lastRenderedFrame = '';

  write = (frame: string) => {
    this.frames.push(frame);
    this.lastRenderedFrame = frame;
    return true;
  };

  lastFrame = () => this.lastRenderedFrame;
}

class Stdin extends EventEmitter {
  isTTY = true;
  private data: string | Buffer | null = null;

  write = (data: string | Buffer) => {
    this.data = data;
    this.emit('readable');
    this.emit('data', data);
    return true;
  };

  setEncoding() {}

  setRawMode() {}

  resume() {}

  pause() {}

  ref() {}

  unref() {}

  read = () => {
    const next = this.data;
    this.data = null;
    return next;
  };
}

const inkInstances: Array<ReturnType<typeof inkRender>> = [];

export function renderInk(tree: React.ReactElement) {
  const stdout = new Stdout();
  const stderr = new Stderr();
  const stdin = new Stdin();
  const instance = inkRender(tree, {
    stdout: stdout as unknown as NodeJS.WriteStream,
    stderr: stderr as unknown as NodeJS.WriteStream,
    stdin: stdin as unknown as NodeJS.ReadStream,
    debug: true,
    exitOnCtrlC: false,
    patchConsole: false,
    maxFps: 1000,
  });

  inkInstances.push(instance);

  return {
    rerender: instance.rerender,
    unmount: instance.unmount,
    cleanup: instance.cleanup,
    stdout,
    stderr,
    stdin,
    frames: stdout.frames,
    lastFrame: stdout.lastFrame,
  };
}

export type InkRenderResult = ReturnType<typeof renderInk>;

export function cleanupInk() {
  for (const instance of inkInstances.splice(0)) {
    instance.unmount();
    instance.cleanup();
  }
}

export async function flushInk() {
  await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
  await Promise.resolve();
}

export async function pressInkKey(
  rendered: InkRenderResult,
  key: string | Buffer
) {
  await flushInk();
  rendered.stdin.write(key);
  await flushInk();
}

export async function submitInk(rendered: InkRenderResult, value: string) {
  for (const char of value) {
    await pressInkKey(rendered, char);
  }

  await pressInkKey(rendered, '\r');
}
