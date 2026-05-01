#!/usr/bin/env node

import { mkdir, rename, rm, stat, writeFile } from 'node:fs/promises';
import { createWriteStream } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';

const DEFAULT_CSV_SOURCE_URL =
  'https://huggingface.co/datasets/Aman279/Locomo/resolve/main/locomo.csv';
const DEFAULT_BENCHMARK_SOURCE_URL =
  'https://raw.githubusercontent.com/Backboard-io/Backboard-Locomo-Benchmark/main/locomo_dataset.json';
const DEFAULT_CACHE_DIR = '.cache/locomo';
const EXPECTED_HEADER = 'dialogue_id,turns';

const args = new Set(process.argv.slice(2));
const force = args.has('--force');
const csvSourceUrl = process.env.LOCOMO_DATASET_URL || DEFAULT_CSV_SOURCE_URL;
const benchmarkSourceUrl = process.env.LOCOMO_BENCHMARK_DATASET_URL || DEFAULT_BENCHMARK_SOURCE_URL;
const cacheDir = resolve(process.env.LOCOMO_CACHE_DIR || DEFAULT_CACHE_DIR);
const csvFile = resolve(cacheDir, 'locomo.csv');
const benchmarkFile = resolve(cacheDir, 'locomo_dataset.json');
const manifestFile = resolve(cacheDir, 'manifest.json');

if (args.has('--help') || args.has('-h')) {
  console.log(`Download LOCOMO data into a local cache.

Usage:
  bun tools/memory-eval/download-locomo.mjs [--force]

Environment:
  LOCOMO_DATASET_URL            Override the conversation CSV URL.
  LOCOMO_BENCHMARK_DATASET_URL  Override the labeled benchmark JSON URL.
  LOCOMO_CACHE_DIR              Override the cache directory. Defaults to ${DEFAULT_CACHE_DIR}.
`);
  process.exit(0);
}

await mkdir(cacheDir, { recursive: true });

if (!force && (await exists(csvFile)) && (await exists(benchmarkFile))) {
  const cachedCsv = await stat(csvFile);
  const cachedBenchmark = await stat(benchmarkFile);
  console.log(
    `LOCOMO datasets already cached at ${relativePath(csvFile)} (${cachedCsv.size} bytes) and ${relativePath(benchmarkFile)} (${cachedBenchmark.size} bytes).`
  );
  console.log('Use --force to refresh it.');
  process.exit(0);
}

const csvDownload = await downloadFile({
  url: csvSourceUrl,
  destination: csvFile,
  force,
  validate: async (path) => {
    const header = await readPrefix(path, EXPECTED_HEADER.length);
    if (header !== EXPECTED_HEADER) {
      throw new Error(
        `Downloaded LOCOMO CSV did not start with ${JSON.stringify(EXPECTED_HEADER)}; got ${JSON.stringify(header)}`
      );
    }
  },
});

const benchmarkDownload = await downloadFile({
  url: benchmarkSourceUrl,
  destination: benchmarkFile,
  force,
  validate: validateBenchmarkJson,
});

const manifest = {
  dataset: 'LOCOMO',
  csv: {
    sourceUrl: csvSourceUrl,
    cachedFile: relativePath(csvFile),
    bytes: csvDownload.bytes,
    etag: csvDownload.etag,
    xRepoCommit: csvDownload.xRepoCommit,
    xLinkedEtag: csvDownload.xLinkedEtag,
    format: 'csv',
    expectedHeader: EXPECTED_HEADER,
  },
  benchmark: {
    sourceUrl: benchmarkSourceUrl,
    cachedFile: relativePath(benchmarkFile),
    bytes: benchmarkDownload.bytes,
    etag: benchmarkDownload.etag,
    xRepoCommit: benchmarkDownload.xRepoCommit,
    xLinkedEtag: benchmarkDownload.xLinkedEtag,
    format: 'json',
  },
  downloadedAt: new Date().toISOString(),
  license: 'not declared by Aman279/Locomo on Hugging Face; verify upstream terms before redistribution',
  redistribution: 'cached locally only; do not commit dataset files to this repository',
};

await writeFile(manifestFile, `${JSON.stringify(manifest, null, 2)}\n`);

console.log(`Cached LOCOMO CSV at ${relativePath(csvFile)} (${csvDownload.bytes} bytes).`);
console.log(
  `Cached LOCOMO benchmark JSON at ${relativePath(benchmarkFile)} (${benchmarkDownload.bytes} bytes).`
);
console.log(`Wrote manifest at ${relativePath(manifestFile)}.`);

async function downloadFile({ url, destination, force: forceDownload, validate }) {
  if (!forceDownload && (await exists(destination))) {
    const cached = await stat(destination);
    return {
      bytes: cached.size,
      etag: null,
      xRepoCommit: null,
      xLinkedEtag: null,
      fromCache: true,
    };
  }

  const tempFile = `${destination}.tmp`;
  console.log(`Downloading ${url}`);
  const response = await fetch(url, {
    headers: {
      'user-agent': 'animaOS-SWARM memory benchmark downloader',
    },
  });

  if (!response.ok || !response.body) {
    throw new Error(`Failed to download ${url}: HTTP ${response.status} ${response.statusText}`);
  }

  await mkdir(dirname(tempFile), { recursive: true });
  await pipeline(Readable.fromWeb(response.body), createWriteStream(tempFile));

  const downloaded = await stat(tempFile);
  if (downloaded.size === 0) {
    await rm(tempFile, { force: true });
    throw new Error(`Downloaded ${url} was empty.`);
  }

  try {
    await validate(tempFile);
  } catch (error) {
    await rm(tempFile, { force: true });
    throw error;
  }

  await rename(tempFile, destination);

  return {
    bytes: downloaded.size,
    etag: response.headers.get('etag'),
    xRepoCommit: response.headers.get('x-repo-commit'),
    xLinkedEtag: response.headers.get('x-linked-etag'),
    fromCache: false,
  };
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

async function readPrefix(path, length) {
  const file = await import('node:fs/promises').then((fs) => fs.open(path, 'r'));
  try {
    const buffer = Buffer.alloc(length);
    await file.read(buffer, 0, length, 0);
    return buffer.toString('utf8');
  } finally {
    await file.close();
  }
}

function relativePath(path) {
  return path.replace(`${process.cwd()}\\`, '').replace(`${process.cwd()}/`, '').replaceAll('\\', '/');
}

async function validateBenchmarkJson(path) {
  const text = await import('node:fs/promises').then((fs) => fs.readFile(path, 'utf8'));
  const data = JSON.parse(text);
  if (!Array.isArray(data) || data.length < 10) {
    throw new Error('LOCOMO benchmark JSON must contain at least 10 conversations.');
  }
  let labeledQuestionCount = 0;
  for (const item of data) {
    if (!item || !Array.isArray(item.qa) || typeof item.conversation !== 'object') {
      throw new Error('LOCOMO benchmark JSON rows must contain qa and conversation fields.');
    }
    for (const qa of item.qa) {
      if (!qa?.question || !Array.isArray(qa.evidence)) {
        throw new Error('LOCOMO benchmark JSON QA rows must contain question and evidence fields.');
      }
      if (qa.category >= 1 && qa.category <= 4 && qa.evidence.length > 0) {
        labeledQuestionCount += 1;
      }
    }
  }
  if (labeledQuestionCount < 1500) {
    throw new Error(
      `LOCOMO benchmark JSON must contain at least 1500 labeled category 1-4 questions; got ${labeledQuestionCount}.`
    );
  }
}
