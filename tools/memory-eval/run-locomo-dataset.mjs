#!/usr/bin/env node

import { spawnSync } from 'node:child_process';
import { stat } from 'node:fs/promises';
import { resolve } from 'node:path';

const cacheDir = resolve(process.env.LOCOMO_CACHE_DIR || '.cache/locomo');
const datasetPath = resolve(process.env.LOCOMO_DATASET_JSON || `${cacheDir}/locomo_dataset.json`);

if (!(await exists(datasetPath))) {
  console.log('LOCOMO benchmark JSON is missing; fetching datasets first.');
  run('bun', ['tools/memory-eval/download-locomo.mjs'], process.env);
}

const cargoArgs = [
  'test',
  '--manifest-path',
  'Cargo.toml',
  '--target-dir',
  'target/core-rust/memory-locomo-dataset',
  '-p',
  'anima-memory',
  '--features',
  'locomo-eval',
  '--test',
  'locomo_dataset',
  'locomo_dataset_benchmark_reaches_retrieval_thresholds',
  '--',
  '--nocapture',
];

run('cargo', cargoArgs, {
  ...process.env,
  LOCOMO_DATASET_JSON: datasetPath,
});

function run(command, args, env) {
  const result = spawnSync(command, args, {
    cwd: process.cwd(),
    env,
    stdio: 'inherit',
    shell: process.platform === 'win32',
  });

  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

async function exists(path) {
  try {
    await stat(path);
    return true;
  } catch (error) {
    if (error && error.code === 'ENOENT') {
      return false;
    }
    throw error;
  }
}
