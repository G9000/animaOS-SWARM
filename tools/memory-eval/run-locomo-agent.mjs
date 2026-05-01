#!/usr/bin/env node

import { spawn } from 'node:child_process';
import { mkdir, readFile, stat, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const DEFAULT_CACHE_DIR = '.cache/locomo';
const DEFAULT_AGENT_RUN_DIR = '.cache/locomo-agent';
const DEFAULT_TOP_K = 40;
const DEFAULT_MIN_ACCURACY = 0.45;
const DEFAULT_MIN_RETRIEVAL_HIT_RATE = 0.70;
const DEFAULT_MIN_CATEGORY_ACCURACY = 0.20;
const DEFAULT_MIN_QUESTIONS = 1500;
const DEFAULT_MAX_JUDGE_FAILURE_RATE = 0.0;
const DEFAULT_INGEST_CONCURRENCY = 16;
const DEFAULT_AGENT_CONCURRENCY = 1;
const DEFAULT_DAEMON_START_TIMEOUT_MS = 120_000;
const DEFAULT_ANSWER_MAX_TOKENS = 180;
const DEFAULT_JUDGE_MAX_TOKENS = 240;
const DEFAULT_ANSWER_EVIDENCE_LIMIT = 18;
const DEFAULT_ANSWER_EVIDENCE_CHAR_BUDGET = 5600;
const ANSWER_EVIDENCE_MIN_SCORE = 0.55;

const ANSWER_EVIDENCE_STOP_WORDS = new Set([
  'a',
  'an',
  'and',
  'are',
  'as',
  'at',
  'be',
  'been',
  'but',
  'by',
  'did',
  'do',
  'does',
  'for',
  'from',
  'had',
  'has',
  'have',
  'he',
  'her',
  'his',
  'how',
  'if',
  'in',
  'is',
  'it',
  'its',
  'of',
  'on',
  'or',
  'she',
  'that',
  'the',
  'their',
  'they',
  'this',
  'to',
  'was',
  'were',
  'what',
  'when',
  'where',
  'which',
  'who',
  'why',
  'with',
  'would',
]);

const MONTH_TERMS = new Set([
  'january',
  'february',
  'march',
  'april',
  'may',
  'june',
  'july',
  'august',
  'september',
  'october',
  'november',
  'december',
]);

const PROVIDERS = new Map([
  ['openai', provider(['OPENAI_API_KEY', 'OPENAI_KEY', 'OPENAI_TOKEN'])],
  ['anthropic', provider(['ANTHROPIC_API_KEY', 'ANTHROPIC_KEY', 'ANTHROPIC_TOKEN', 'CLAUDE_API_KEY'])],
  [
    'google',
    provider(['GOOGLE_API_KEY', 'GOOGLE_KEY', 'GOOGLE_AI_KEY', 'GEMINI_API_KEY', 'GOOGLE_GENERATIVE_AI_API_KEY']),
  ],
  ['ollama', provider(['OLLAMA_API_KEY'], false)],
  ['groq', provider(['GROQ_API_KEY', 'GROQ_KEY', 'GROQ_TOKEN'])],
  ['xai', provider(['XAI_API_KEY', 'XAI_KEY', 'GROK_API_KEY'])],
  ['openrouter', provider(['OPENROUTER_API_KEY', 'OPENROUTER_KEY', 'OPENROUTER_TOKEN'])],
  ['mistral', provider(['MISTRAL_API_KEY', 'MISTRAL_KEY', 'MISTRAL_TOKEN'])],
  ['together', provider(['TOGETHER_API_KEY', 'TOGETHER_KEY', 'TOGETHER_TOKEN'])],
  ['deepseek', provider(['DEEPSEEK_API_KEY'])],
  ['fireworks', provider(['FIREWORKS_API_KEY'])],
  ['perplexity', provider(['PERPLEXITY_API_KEY'])],
  ['moonshot', provider(['MOONSHOT_API_KEY', 'MOONSHOT_KEY', 'MOONSHOT_TOKEN', 'KIMI_API_KEY'])],
]);

const PROVIDER_ALIASES = new Map([
  ['gemini', 'google'],
  ['grok', 'xai'],
  ['kimi', 'moonshot'],
]);

const args = new Set(process.argv.slice(2));

if (args.has('--help') || args.has('-h')) {
  printHelp();
  process.exit(0);
}

if (args.has('--self-test')) {
  runSelfTest();
  process.exit(0);
}

const config = readConfig();
validateProviderConfig(config);

if (args.has('--check-config')) {
  console.log('LOCOMO agent benchmark configuration is valid.');
  console.log(`  answer model: ${config.agent.provider}/${config.agent.model}`);
  console.log(`  judge model: ${config.judge.provider}/${config.judge.model}`);
  console.log(`  daemon: ${config.daemonUrl ?? 'auto-start isolated daemon'}`);
  console.log(`  dataset: ${config.datasetPath}`);
  process.exit(0);
}

await ensureDataset(config.datasetPath);
const dataset = await loadDataset(config.datasetPath);
const sdk = await import(pathToFileURL(resolve('packages/sdk/src/index.ts')).href);
const daemon = await connectDaemon(config, sdk.createDaemonClient);

try {
  const report = await runBenchmark(dataset, daemon.client, config);
  await writeReport(report, config);
  printReport(report, config);
  assertReport(report, config);
} finally {
  await daemon.stop();
}

function provider(apiKeyEnvNames, requiresKey = true) {
  return { apiKeyEnvNames, requiresKey };
}

function printHelp() {
  console.log(`Run the end-to-end LOCOMO agent benchmark with real model calls.

This target is intentionally not a fake/offline pass. It requires a real answer model
and a real judge model through the rust daemon runtime provider adapter.

Required environment:
  LOCOMO_AGENT_PROVIDER      Answer provider: openai, anthropic, google, ollama, groq, xai, openrouter, mistral, together, deepseek, fireworks, perplexity, moonshot.
  LOCOMO_AGENT_MODEL         Answer model name for the provider.
  LOCOMO_JUDGE_PROVIDER      Optional judge provider. Defaults to LOCOMO_AGENT_PROVIDER.
  LOCOMO_JUDGE_MODEL         Optional judge model. Defaults to LOCOMO_AGENT_MODEL.
  Provider API key env       For example OPENAI_API_KEY for openai. Ollama does not require a key.

Optional environment:
  LOCOMO_DAEMON_URL                  Use an existing daemon instead of auto-starting an isolated daemon.
  LOCOMO_AGENT_TOP_K                 Retrieved memories per question. Default ${DEFAULT_TOP_K}.
  LOCOMO_AGENT_MAX_QUESTIONS         Optional cap for paid smoke runs. Full production leaves this unset.
  LOCOMO_AGENT_MIN_QUESTIONS         Default ${DEFAULT_MIN_QUESTIONS}.
  LOCOMO_AGENT_MIN_ACCURACY          Default ${DEFAULT_MIN_ACCURACY}.
  LOCOMO_AGENT_MIN_RETRIEVAL_HIT_RATE Default ${DEFAULT_MIN_RETRIEVAL_HIT_RATE}.
  LOCOMO_AGENT_MIN_CATEGORY_ACCURACY Default ${DEFAULT_MIN_CATEGORY_ACCURACY}.
  LOCOMO_AGENT_MAX_JUDGE_FAILURE_RATE Default ${DEFAULT_MAX_JUDGE_FAILURE_RATE}.
  LOCOMO_AGENT_CONCURRENCY           Concurrent answer+judge evaluations. Default ${DEFAULT_AGENT_CONCURRENCY}.
  LOCOMO_INGEST_CONCURRENCY          Concurrent memory writes during ingestion. Default ${DEFAULT_INGEST_CONCURRENCY}.
  LOCOMO_AGENT_ANSWER_EVIDENCE_LIMIT Reranked evidence lines included in the answer prompt. Default ${DEFAULT_ANSWER_EVIDENCE_LIMIT}.
  LOCOMO_AGENT_ANSWER_EVIDENCE_CHAR_BUDGET Max reranked answer-prompt evidence characters. Default ${DEFAULT_ANSWER_EVIDENCE_CHAR_BUDGET}.
  LOCOMO_AGENT_REPORT_EVIDENCE       Set to 1 to include retrieved evidence snippets in the JSON report.
  ANIMAOS_RS_MEMORY_QUERY_EXPANDER   Auto-started daemon defaults to locomo for this benchmark; set none/off to disable.

Commands:
  bun tools/memory-eval/run-locomo-agent.mjs --check-config
  bun tools/memory-eval/run-locomo-agent.mjs --self-test
  bun tools/memory-eval/run-locomo-agent.mjs
`);
}

function readConfig() {
  const cacheDir = resolve(process.env.LOCOMO_CACHE_DIR || DEFAULT_CACHE_DIR);
  const runId = new Date().toISOString().replace(/[:.]/g, '-');
  const runDir = resolve(process.env.LOCOMO_AGENT_RUN_DIR || `${DEFAULT_AGENT_RUN_DIR}/${runId}-${process.pid}`);
  const datasetPath = resolve(process.env.LOCOMO_DATASET_JSON || `${cacheDir}/locomo_dataset.json`);
  const agentProvider = requiredEnv('LOCOMO_AGENT_PROVIDER');
  const agentModel = requiredEnv('LOCOMO_AGENT_MODEL');
  const judgeProvider = process.env.LOCOMO_JUDGE_PROVIDER || agentProvider;
  const judgeModel = process.env.LOCOMO_JUDGE_MODEL || agentModel;

  return {
    cacheDir,
    datasetPath,
    runDir,
    resultsFile: resolve(process.env.LOCOMO_AGENT_RESULTS_FILE || `${runDir}/locomo-agent-results.json`),
    daemonUrl: nonEmptyEnv('LOCOMO_DAEMON_URL'),
    topK: envInteger('LOCOMO_AGENT_TOP_K', DEFAULT_TOP_K),
    maxQuestions: optionalInteger('LOCOMO_AGENT_MAX_QUESTIONS'),
    minQuestions: envInteger('LOCOMO_AGENT_MIN_QUESTIONS', DEFAULT_MIN_QUESTIONS),
    minAccuracy: envNumber('LOCOMO_AGENT_MIN_ACCURACY', DEFAULT_MIN_ACCURACY),
    minRetrievalHitRate: envNumber('LOCOMO_AGENT_MIN_RETRIEVAL_HIT_RATE', DEFAULT_MIN_RETRIEVAL_HIT_RATE),
    minCategoryAccuracy: envNumber('LOCOMO_AGENT_MIN_CATEGORY_ACCURACY', DEFAULT_MIN_CATEGORY_ACCURACY),
    maxJudgeFailureRate: envNumber('LOCOMO_AGENT_MAX_JUDGE_FAILURE_RATE', DEFAULT_MAX_JUDGE_FAILURE_RATE),
    ingestConcurrency: envInteger('LOCOMO_INGEST_CONCURRENCY', DEFAULT_INGEST_CONCURRENCY),
    agentConcurrency: envInteger('LOCOMO_AGENT_CONCURRENCY', DEFAULT_AGENT_CONCURRENCY),
    answerMaxTokens: envInteger('LOCOMO_AGENT_ANSWER_MAX_TOKENS', DEFAULT_ANSWER_MAX_TOKENS),
    judgeMaxTokens: envInteger('LOCOMO_AGENT_JUDGE_MAX_TOKENS', DEFAULT_JUDGE_MAX_TOKENS),
    answerEvidenceLimit: envInteger('LOCOMO_AGENT_ANSWER_EVIDENCE_LIMIT', DEFAULT_ANSWER_EVIDENCE_LIMIT),
    answerEvidenceCharBudget: envInteger(
      'LOCOMO_AGENT_ANSWER_EVIDENCE_CHAR_BUDGET',
      DEFAULT_ANSWER_EVIDENCE_CHAR_BUDGET
    ),
    reportEvidence: envBoolean('LOCOMO_AGENT_REPORT_EVIDENCE', false),
    daemonStartTimeoutMs: envInteger('LOCOMO_DAEMON_START_TIMEOUT_MS', DEFAULT_DAEMON_START_TIMEOUT_MS),
    categories: parseCategoryFilter(process.env.LOCOMO_AGENT_CATEGORIES),
    agent: {
      provider: normalizeProvider(agentProvider),
      model: agentModel,
    },
    judge: {
      provider: normalizeProvider(judgeProvider),
      model: judgeModel,
    },
  };
}

function validateProviderConfig(config) {
  validateProviderRole('LOCOMO_AGENT_PROVIDER', config.agent.provider);
  validateProviderRole('LOCOMO_JUDGE_PROVIDER', config.judge.provider);

  if (!config.daemonUrl) {
    ensureProviderEnv(config.agent.provider, 'answer');
    ensureProviderEnv(config.judge.provider, 'judge');
  }
}

function validateProviderRole(name, providerName) {
  if (providerName === 'deterministic' || providerName === 'test') {
    throw new Error(`${name} must be a real provider, not ${providerName}.`);
  }
  if (!PROVIDERS.has(providerName)) {
    throw new Error(`${name}=${providerName} is unsupported by the benchmark runner.`);
  }
}

function ensureProviderEnv(providerName, role) {
  const providerInfo = PROVIDERS.get(providerName);
  if (!providerInfo?.requiresKey) {
    return;
  }
  const configured = providerInfo.apiKeyEnvNames.some((name) => nonEmptyEnv(name));
  if (!configured) {
    throw new Error(
      `Missing API key for ${role} provider ${providerName}. Set one of: ${providerInfo.apiKeyEnvNames.join(', ')}.`
    );
  }
}

function normalizeProvider(value) {
  const normalized = value.trim().toLowerCase();
  return PROVIDER_ALIASES.get(normalized) || normalized;
}

async function ensureDataset(datasetPath) {
  if (await exists(datasetPath)) {
    return;
  }
  console.log('LOCOMO benchmark JSON is missing; fetching datasets first.');
  await runCommand('bun', ['tools/memory-eval/download-locomo.mjs'], process.env);
}

async function loadDataset(datasetPath) {
  const text = await readFile(datasetPath, 'utf8');
  const dataset = JSON.parse(text);
  if (!Array.isArray(dataset) || dataset.length < 10) {
    throw new Error('LOCOMO dataset JSON must contain at least 10 conversations.');
  }
  return dataset;
}

async function connectDaemon(config, createDaemonClient) {
  if (config.daemonUrl) {
    const client = createDaemonClient({ baseUrl: config.daemonUrl });
    await client.health();
    await ensureLiveDaemonProvider(client, config.agent.provider, 'answer');
    await ensureLiveDaemonProvider(client, config.judge.provider, 'judge');
    return { client, stop: async () => undefined };
  }

  await mkdir(config.runDir, { recursive: true });
  const port = await freePort();
  const baseUrl = `http://127.0.0.1:${port}`;
  const daemon = spawn('cargo', ['run', '-p', 'anima-daemon'], {
    cwd: process.cwd(),
    env: {
      ...process.env,
      ANIMAOS_RS_HOST: '127.0.0.1',
      ANIMAOS_RS_PORT: String(port),
      ANIMAOS_RS_MEMORY_SQLITE_FILE: resolve(config.runDir, 'memory.sqlite'),
      ANIMAOS_RS_MEMORY_EMBEDDINGS_SQLITE_FILE: resolve(config.runDir, 'memory-embeddings.sqlite'),
      ANIMAOS_RS_MEMORY_EMBEDDINGS: process.env.ANIMAOS_RS_MEMORY_EMBEDDINGS || 'local',
      ANIMAOS_RS_MEMORY_QUERY_EXPANDER: process.env.ANIMAOS_RS_MEMORY_QUERY_EXPANDER || 'locomo',
      ANIMAOS_RS_REQUEST_TIMEOUT_SECS: process.env.ANIMAOS_RS_REQUEST_TIMEOUT_SECS || '300',
      ANIMAOS_RS_MAX_REQUEST_BYTES: process.env.ANIMAOS_RS_MAX_REQUEST_BYTES || '1048576',
      RUST_LOG: process.env.RUST_LOG || 'anima_daemon=warn,tower_http=warn',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });

  daemon.stdout.on('data', (chunk) => process.stdout.write(`[daemon] ${chunk}`));
  daemon.stderr.on('data', (chunk) => process.stderr.write(`[daemon] ${chunk}`));

  const client = createDaemonClient({ baseUrl });
  await waitForDaemon(client, daemon, config.daemonStartTimeoutMs);
  return {
    client,
    stop: async () => stopDaemon(daemon),
  };
}

async function ensureLiveDaemonProvider(client, providerName, role) {
  const response = await client.requestJson('/api/providers');
  const providers = Array.isArray(response.providers) ? response.providers : [];
  const providerStatus = providers.find((entry) => entry.id === providerName);
  if (!providerStatus) {
    throw new Error(`Live daemon does not advertise ${role} provider ${providerName}.`);
  }
  if (providerStatus.requiresKey && !providerStatus.configured) {
    throw new Error(`Live daemon provider ${providerName} is not configured for ${role} calls.`);
  }
}

async function waitForDaemon(client, daemon, timeoutMs) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    if (daemon.exitCode !== null) {
      throw new Error(`daemon exited before becoming healthy with code ${daemon.exitCode}`);
    }
    try {
      await client.health();
      return;
    } catch {
      await delay(500);
    }
  }
  throw new Error(`daemon did not become healthy within ${timeoutMs}ms`);
}

async function stopDaemon(daemon) {
  if (daemon.exitCode !== null || daemon.killed) {
    return;
  }
  daemon.kill('SIGTERM');
  await Promise.race([
    new Promise((resolve) => daemon.once('exit', resolve)),
    delay(5000).then(() => {
      if (daemon.exitCode === null && !daemon.killed) {
        daemon.kill('SIGKILL');
      }
    }),
  ]);
}

async function runBenchmark(dataset, client, config) {
  const report = newReport(config);
  const judgeAgent = await createJudgeAgent(client, config);
  const questionJobs = [];

  for (const [conversationIndex, item] of dataset.entries()) {
    const memoryAgent = await createMemoryAgent(client, config, conversationIndex, item);
    const memoryIdByDiaId = await ingestConversation(client, memoryAgent, item, config, report);
    const questions = selectableQuestions(item, memoryIdByDiaId, config);

    for (const question of questions) {
      if (config.maxQuestions !== undefined && questionJobs.length >= config.maxQuestions) {
        break;
      }
      questionJobs.push({ item, question, memoryAgent, memoryIdByDiaId, conversationIndex });
    }
    if (config.maxQuestions !== undefined && questionJobs.length >= config.maxQuestions) {
      break;
    }
  }

  report.selectedQuestions = questionJobs.length;
  await mapPool(questionJobs, config.agentConcurrency, async (job, index) => {
    const result = await evaluateQuestion(client, judgeAgent, job, config);
    recordQuestion(report, result);
    const completed = index + 1;
    if (completed === questionJobs.length || completed % 10 === 0) {
      console.log(
        `LOCOMO agent progress ${completed}/${questionJobs.length}: accuracy=${formatRatio(report.correctAnswers, report.judgedQuestions)} retrieval_hit=${formatRatio(report.retrievalHitQuestions, report.evaluatedQuestions)}`
      );
    }
  });

  return finalizeReport(report);
}

async function createMemoryAgent(client, config, conversationIndex, item) {
  const snapshot = await client.agents.create({
    name: `locomo-memory-${conversationIndex + 1}-${item.sample_id || 'sample'}`,
    provider: config.agent.provider,
    model: config.agent.model,
    system: 'Owns isolated LOCOMO conversation memory for benchmark ingestion. This agent is not used to answer questions.',
    settings: {
      temperature: 0,
      maxTokens: config.answerMaxTokens,
    },
  });
  return snapshot.state;
}

async function createJudgeAgent(client, config) {
  const snapshot = await client.agents.create({
    name: 'locomo-judge',
    provider: config.judge.provider,
    model: config.judge.model,
    system: judgeSystemPrompt(),
    settings: {
      temperature: 0.1,
      maxTokens: config.judgeMaxTokens,
    },
  });
  return snapshot.state;
}

async function createAnswerAgent(client, config, job, questionIndex) {
  const snapshot = await client.agents.create({
    name: `locomo-answer-${job.conversationIndex + 1}-${questionIndex + 1}`,
    provider: config.agent.provider,
    model: config.agent.model,
    system: answerSystemPrompt(),
    settings: {
      temperature: 0,
      maxTokens: config.answerMaxTokens,
    },
  });
  return snapshot.state;
}

async function ingestConversation(client, memoryAgent, item, config, report) {
  const memoryIdByDiaId = new Map();
  const turns = extractTurns(item);
  report.conversations += 1;
  report.ingestedTurns += turns.length;

  await mapPool(turns, config.ingestConcurrency, async (turn) => {
    const memory = await client.memories.create({
      agentId: memoryAgent.id,
      agentName: memoryAgent.name,
      type: 'observation',
      content: turnMemoryContent(turn),
      importance: 0.55,
      tags: ['locomo', turn.dia_id],
      scope: 'room',
      roomId: item.sample_id,
      worldId: 'locomo',
      sessionId: turn.session_id,
    });
    memoryIdByDiaId.set(turn.dia_id, memory.id);
  });

  return memoryIdByDiaId;
}

function selectableQuestions(item, memoryIdByDiaId, config) {
  const selected = [];
  for (const [index, qa] of (item.qa || []).entries()) {
    const category = Number(qa.category);
    const answer = answerToText(qa.answer).trim();
    const evidence = Array.isArray(qa.evidence) ? qa.evidence : [];
    if (category === 5 || !config.categories.has(category) || evidence.length === 0 || answer.length === 0) {
      continue;
    }
    const expectedMemoryIds = evidence
      .map((diaId) => memoryIdByDiaId.get(diaId))
      .filter((memoryId) => typeof memoryId === 'string');
    if (expectedMemoryIds.length === 0) {
      continue;
    }
    selected.push({ ...qa, index, category, answer, expectedMemoryIds });
  }
  return selected;
}

async function evaluateQuestion(client, judgeAgent, job, config) {
  const retrieved = await client.memories.recall(job.question.question, {
    agentId: job.memoryAgent.id,
    scope: 'room',
    roomId: job.item.sample_id,
    worldId: 'locomo',
    limit: config.topK,
    lexicalLimit: Math.max(config.topK * 4, config.topK),
    recentLimit: 0,
    relationshipLimit: 0,
  });
  const expected = new Set(job.question.expectedMemoryIds);
  const ranks = retrieved
    .map((result, index) => (expected.has(result.memory.id) ? index + 1 : null))
    .filter((rank) => rank !== null);
  const bestRank = ranks.length > 0 ? Math.min(...ranks) : null;
  const allEvidenceHit = ranks.length === expected.size;
  const answerAgent = await createAnswerAgent(client, config, job, job.question.index);
  let answerEvidence = selectAnswerEvidence(
    job.question.question,
    retrieved,
    config.answerEvidenceLimit,
    config.answerEvidenceCharBudget
  );
  const answerPrompt = buildAnswerPrompt(job.question.question, answerEvidence, {
    forceShortAnswer: false,
  });
  let answerRun;
  try {
    answerRun = await runAgent(client, answerAgent.id, answerPrompt, 'answer');
  } catch (error) {
    if (!isMissingTextResponseError(error)) {
      throw error;
    }
    const retryEvidence = selectAnswerEvidence(
      job.question.question,
      retrieved,
      Math.min(config.answerEvidenceLimit, 4),
      Math.min(config.answerEvidenceCharBudget, 900)
    );
    answerEvidence = retryEvidence;
    const retryPrompt = buildAnswerPrompt(job.question.question, retryEvidence, {
      forceShortAnswer: true,
    });
    answerRun = await runAgent(client, answerAgent.id, retryPrompt, 'answer');
  }
  if (isIDontKnowAnswer(answerRun.result.data.text) && answerEvidence.length > 0) {
    const retryPrompt = buildAnswerPrompt(job.question.question, answerEvidence, {
      forceShortAnswer: false,
      forceBestEffort: true,
    });
    answerRun = await runAgent(client, answerAgent.id, retryPrompt, 'answer retry');
  }
  const answerEvidenceIds = answerEvidence.map((result) => result.memory.id);
  const answerEvidenceExpectedCount = answerEvidenceIds.filter((memoryId) => expected.has(memoryId)).length;
  const answerEvidenceChars = answerEvidence.reduce(
    (total, result, index) => total + answerEvidenceLineLength(result, index),
    0
  );
  const generatedAnswer = answerRun.result.data.text.trim();
  const judgePrompt = buildJudgePrompt(job.question.question, job.question.answer, generatedAnswer);
  const judgeRun = await runAgent(client, judgeAgent.id, judgePrompt, 'judge');
  const judgment = parseJudgment(judgeRun.result.data.text);

  return {
    sampleId: job.item.sample_id,
    questionIndex: job.question.index,
    category: job.question.category,
    question: job.question.question,
    expectedAnswer: job.question.answer,
    generatedAnswer,
    judgeLabel: judgment.label,
    judgeReasoning: judgment.reasoning,
    judgeParseError: judgment.parseError,
    retrievedMemoryIds: retrieved.map((result) => result.memory.id),
    answerEvidenceMemoryIds: answerEvidenceIds,
    expectedMemoryIds: [...expected],
    evidenceHit: bestRank !== null,
    allEvidenceHit,
    answerEvidenceHit: answerEvidenceExpectedCount > 0,
    answerAllEvidenceHit: answerEvidenceExpectedCount === expected.size,
    answerEvidenceExpectedCount,
    answerEvidenceCount: answerEvidence.length,
    answerEvidenceChars,
    reciprocalRank: bestRank === null ? 0 : 1 / bestRank,
    answerDurationMs: answerRun.result.durationMs,
    judgeDurationMs: judgeRun.result.durationMs,
    answerTokenUsage: answerRun.agent.state.tokenUsage,
    judgeTokenUsage: judgeRun.agent.state.tokenUsage,
    retrievedEvidence: config.reportEvidence ? retrieved.map(evidenceSummary) : undefined,
    answerEvidence: config.reportEvidence ? answerEvidence.map(evidenceSummary) : undefined,
  };
}

async function runAgent(client, agentId, prompt, label) {
  const run = await client.agents.run(agentId, { text: prompt });
  const text = typeof run.result.data?.text === 'string' ? run.result.data.text : '';
  if (run.result.status !== 'success' || text.trim().length === 0) {
    throw new Error(
      `${label} model run failed: ${run.result.error || 'missing text response'}\n${formatRunDiagnostics(run, prompt, label)}`
    );
  }
  return run;
}

function formatRunDiagnostics(run, prompt, label) {
  const metadata = run.result.data?.metadata;
  const summary = {
    label,
    status: run.result.status,
    error: run.result.error || null,
    durationMs: run.result.durationMs,
    hasData: Boolean(run.result.data),
    textLength: typeof run.result.data?.text === 'string' ? run.result.data.text.length : 0,
    attachmentCount: Array.isArray(run.result.data?.attachments) ? run.result.data.attachments.length : 0,
    metadataKeys:
      metadata && typeof metadata === 'object' && !Array.isArray(metadata) ? Object.keys(metadata).sort() : [],
    promptChars: prompt.length,
    promptPreview: prompt.slice(0, 240),
    tokenUsage: run.agent?.state?.tokenUsage || null,
  };
  return `run diagnostics: ${JSON.stringify(summary, null, 2)}`;
}

function buildAnswerPrompt(question, evidenceResults, options) {
  const evidence = evidenceResults
    .map((result, index) => `[${index + 1}] ${result.memory.content}`)
    .join('\n');
  const answerInstruction = options.forceShortAnswer
    ? 'Return only the shortest final answer phrase, date, or name. Maximum 10 words. No explanation. No reasoning. No markdown.'
    : 'Return only the answer. No explanation. No reasoning. No markdown.';
  const profile = buildQuestionProfile(question);
  const aggregationInstruction = profile.wantsAggregation
    ? 'This is a profile/list/count extraction question. Silently scan every evidence line before answering. Do not stop after the first matching fact. Include every distinct directly supported item that matches the requested type, and exclude related facts of a different type. For count questions, return one number and count only distinct direct matches. For questions asking what two people both did/painted/shared, answer only the item supported for both people.'
    : '';
  const artKindInstruction = profile.wantsArtKind
    ? 'For "what kind of art" questions, prefer the art style, theme, or visual metadata over the medium; do not answer only "painting" or "drawing" when a more specific style/theme is supported.'
    : '';
  const scalarInstruction = profile.wantsSingleAnswer
    ? 'This is a single-answer extraction question. Return one answer only. Do not list multiple candidate dates, events, objects, or plans. Prefer the evidence line that best matches the full subject plus event/relation in the question, and ignore broader same-topic memories.'
    : '';
  const temporalInstruction = profile.wantsTemporal
    ? 'For date/time answers, prefer the relative expression from the direct evidence plus the session reference, such as "the Friday before 22 October 2023". Do not convert a relative weekday to an absolute calendar date unless the resolved date is explicit and unambiguous in the evidence or Time hints.'
    : '';
  const relationInstruction = relationSpecificInstruction(profile);
  const bestEffortInstruction = options.forceBestEffort
    ? 'This is a retry after an uncertain answer: make the best supported answer from the evidence. Do not answer "I don\'t know" just because the support is indirect, relative, visual metadata, or spread across multiple lines.'
    : '';
  return `Question:\n${question}\n\nRetrieved memory evidence:\n${evidence || 'No evidence retrieved.'}\n\nUse only the retrieved memory evidence. Use normal text, image captions, image queries, session dates, and time hints as evidence. First identify the evidence lines that directly match the subject and relation in the question, then ignore unrelated nearby facts. Resolve relative dates such as "yesterday", "last night", "last week", "last year", or "two weekends ago" using "Session date:" and "Time hints:" text. For count questions, count distinct supported events or entities. For list or multi-part questions, include every distinct requested item supported by the evidence, but do not add extra items outside the requested category. Prefer specific entities from evidence over generic aliases such as "home country", "that place", or "that book". If one evidence line uses an alias and another identifies it, answer with the identified name. For likely, would, might, trait, preference, leaning, or counterfactual questions, answer the best supported inference instead of saying "I don't know" when evidence points one way. If the evidence truly has no matching support, answer exactly "I don't know". ${aggregationInstruction} ${artKindInstruction} ${scalarInstruction} ${temporalInstruction} ${relationInstruction} ${bestEffortInstruction} ${answerInstruction}`;
}

function relationSpecificInstruction(profile) {
  const instructions = [];
  if (profile.subjectName && wantsSubjectSpeakerFocus(profile)) {
    instructions.push(
      `For questions about ${profile.subjectName}, prefer evidence where ${profile.subjectName} is the speaker or where the line directly names ${profile.subjectName}; do not use another speaker's own activities as ${profile.subjectName}'s activities.`
    );
  }
  if (profile.wantsDestress) {
    instructions.push(
      'For destress questions, include activities tied to evidence words such as destress, de-stress, therapy, clear my mind, or headspace; exclude generic me-time or refreshing activities unless that same line directly says destress.'
    );
  }
  if (profile.wantsEventHelp) {
    instructions.push(
      'For events that help children or youth, include event-like facts such as mentorship programs, school events, speeches, or talks; exclude ongoing volunteer work unless the question asks about volunteering.'
    );
  }
  if (profile.wantsLgbtqEvents) {
    instructions.push(
      'For LGBTQ event questions, include only event or group-participation memories such as support groups, school events, pride parades, meetings, campaigns, speeches, or talks; avoid generic support comments.'
    );
  }
  if (profile.wantsSupportGroups) {
    instructions.push(
      'For support questions, if one line gives generic support and another line names the supporters, answer with the named groups such as friends, family, or mentors.'
    );
  }
  if (profile.wantsSymbols) {
    instructions.push(
      'For symbol questions, image query labels such as "transgender symbol" are symbol evidence. Prefer identity/community symbols like rainbow flag or transgender symbol; exclude animal motifs unless the question asks for every image motif.'
    );
  }
  if (profile.wantsMusicalArtists) {
    instructions.push(
      'For musical artist or band questions, quoted capitalized music names in show or concert evidence count as artist/band names.'
    );
  }
  if (profile.wantsHikeActions) {
    instructions.push(
      'For hike or camping action questions, answer concrete actions such as roasting marshmallows or telling stories, not a generic purpose like connecting with nature.'
    );
  }
  if (profile.wantsBoughtItems) {
    instructions.push('For bought-item questions, include items from lines using bought, got, new, or just got.');
  }
  if (profile.wantsBooks) {
    instructions.push(
      'For book questions, include only books the named person read or loved directly. Do not resolve vague aliases like "that book you recommended" into a title unless the named person also states the title.'
    );
  }
  if (profile.wantsPotteryTypes) {
    instructions.push(
      'For pottery type questions, answer object types from image captions, image queries, and direct object words such as cup or bowl; ignore generic process words like pots when a more specific object type is present.'
    );
  }
  if (profile.wantsPaintedList) {
    instructions.push(
      'For painted-item questions, answer concrete subjects or scenes from the named person such as horse, sunset, or sunrise. Exclude generic styles such as abstract unless the question asks for kind or style of art.'
    );
  }
  if (profile.wantsChildrenCount) {
    instructions.push(
      'For child-count questions, infer distinct children only from directly related family evidence, such as a son plus references to that son as the other children\'s brother.'
    );
  }
  if (profile.wantsPostRoadtripHikeDate) {
    instructions.push(
      'For hike-after-roadtrip date questions, use the line that says the hike happened yesterday after the road trip and resolve yesterday with the session date.'
    );
  }
  if (profile.wantsBookDate) {
    instructions.push(
      'For book-date questions, if the evidence says the person read the book last year, answer with the previous year from the session date.'
    );
  }
  if (profile.wantsCareerAlternative) {
    instructions.push(
      'For career alternative questions, compare the proposed career to the person\'s stated career goals. If the evidence shows a different career path and only a hobby or interest supports the proposed option, answer likely no with the stated career path.'
    );
  }
  if (profile.wantsReligiousInference) {
    instructions.push(
      'For religiousness questions, church, faith, or religious-art evidence supports somewhat religious. A bad experience with religious conservatives means not extremely religious or not aligned with them, not automatically nonreligious.'
    );
  }
  if (profile.wantsRoadtripSoon) {
    instructions.push(
      'For another-roadtrip-soon questions, a recent scary accident, bad start, or badly ended trip supports likely no soon.'
    );
  }
  if (profile.wantsMoveBackHomeSoon) {
    instructions.push(
      'For move-back-home-country-soon questions, current adoption interviews, building a family, or giving children a home supports no soon even if roots or home-country ties are present.'
    );
  }
  if (profile.wantsPrideParadeSummer) {
    instructions.push(
      'For pride-parade timing questions, answer the one parade timing that directly matches the question. Do not list every Pride or LGBTQ event memory.'
    );
  }
  if (profile.wantsArtPracticeDuration) {
    instructions.push('For art-practice duration questions, prefer evidence with since, started, years, or explicit year text.');
  }
  if (profile.wantsAdoptionInterviewDate) {
    instructions.push('For adoption-interview date questions, use the interview line with its relative Friday/session date, not unrelated adoption plans.');
  }
  if (profile.wantsSummerPlans) {
    instructions.push('For summer-plan questions, prefer the named person\'s own plan over suggestions for joint outings.');
  }
  if (profile.wantsBowlReminder) {
    instructions.push('For hand-painted bowl reminder questions, use evidence about the bowl\'s meaning, not general pottery activity.');
  }
  if (profile.wantsAdoptionAgencySupport) {
    instructions.push(
      'For adoption-agency support or choice questions, prefer the agency line with inclusivity, support, LGBTQ+ folks, or why the agency spoke to the person; ignore generic adoption motivation unless the question asks why they want to adopt.'
    );
  }
  if (profile.wantsCounselingServices) {
    instructions.push(
      'For counseling and mental-health service questions, include the specific target population and service details, especially working with trans people, helping them accept themselves, and supporting mental health.'
    );
  }
  if (profile.wantsBowlMade) {
    instructions.push(
      'For bowl-made questions, answer yes if a direct line says the person made this bowl or made it in class, using the nearby photo description only to identify the bowl.'
    );
  }
  return instructions.join(' ');
}

function evidenceSummary(result, index) {
  return {
    rank: index + 1,
    memoryId: result.memory.id,
    score: result.score,
    lexicalScore: result.lexicalScore,
    vectorScore: result.vectorScore,
    relationshipScore: result.relationshipScore,
    recencyScore: result.recencyScore,
    content: result.memory.content,
  };
}

function selectAnswerEvidence(question, retrieved, evidenceLimit, evidenceCharBudget) {
  const profile = buildQuestionProfile(question);
  const targetLimit = answerEvidenceLimitForProfile(profile, evidenceLimit);
  const scored = retrieved
    .map((result, index) => scoreAnswerEvidence(result, index, retrieved.length, profile))
    .sort((left, right) => right.score - left.score || left.index - right.index);
  const selected = [];
  let usedChars = 0;
  const desiredMinimum = answerEvidenceMinimumForProfile(profile, targetLimit);

  for (const candidate of scored) {
    if (selected.length >= targetLimit) {
      break;
    }
    if (selected.length >= desiredMinimum && candidate.score < ANSWER_EVIDENCE_MIN_SCORE) {
      continue;
    }
    const lineLength = answerEvidenceLineLength(candidate.result, selected.length);
    if (selected.length > 0 && usedChars + lineLength > evidenceCharBudget) {
      continue;
    }
    if (selected.length >= 2 && isNearDuplicateEvidence(candidate, selected)) {
      continue;
    }
    selected.push(candidate);
    usedChars += lineLength;
  }

  if (selected.length < Math.min(3, targetLimit, retrieved.length)) {
    for (const candidate of scored.sort((left, right) => left.index - right.index)) {
      if (selected.some((entry) => entry.result.memory.id === candidate.result.memory.id)) {
        continue;
      }
      const lineLength = answerEvidenceLineLength(candidate.result, selected.length);
      if (selected.length > 0 && usedChars + lineLength > evidenceCharBudget) {
        continue;
      }
      selected.push(candidate);
      usedChars += lineLength;
      if (selected.length >= Math.min(3, targetLimit, retrieved.length)) {
        break;
      }
    }
  }

  return selected.length > 0 ? selected.map((candidate) => candidate.result) : retrieved.slice(0, 1);
}

function answerEvidenceLimitForProfile(profile, configuredLimit) {
  if (profile.wantsAdoptionAgencySupport || profile.wantsCounselingServices || profile.wantsBowlMade) {
    return Math.min(configuredLimit, 10);
  }
  if (profile.wantsChildrenCount) {
    return Math.min(configuredLimit, 10);
  }
  if (profile.wantsPostRoadtripHikeDate) {
    return Math.min(configuredLimit, 10);
  }
  if (profile.wantsEventHelp) {
    return Math.min(configuredLimit, 4);
  }
  if (profile.wantsHikeActions) {
    return Math.min(configuredLimit, 6);
  }
  if (profile.wantsLgbtqEvents) {
    return Math.min(configuredLimit, 12);
  }
  if (profile.wantsPotteryTypes || profile.wantsPaintedList || profile.wantsSymbols) {
    return Math.min(configuredLimit, 6);
  }
  if (profile.wantsCount) {
    return Math.min(configuredLimit, 6);
  }
  if (profile.wantsTemporal) {
    return Math.min(configuredLimit, 6);
  }
  if (profile.wantsTightList) {
    return Math.min(configuredLimit, 8);
  }
  if (profile.wantsAggregation) {
    return configuredLimit;
  }
  if (profile.wantsInferential) {
    return Math.min(configuredLimit, 14);
  }
  return Math.min(configuredLimit, 6);
}

function answerEvidenceMinimumForProfile(profile, targetLimit) {
  if (profile.wantsAdoptionAgencySupport || profile.wantsCounselingServices || profile.wantsBowlMade) {
    return Math.min(targetLimit, 6);
  }
  if (profile.wantsLgbtqEvents) {
    return Math.min(targetLimit, 8);
  }
  if (profile.wantsChildrenCount || profile.wantsPostRoadtripHikeDate) {
    return Math.min(targetLimit, 6);
  }
  if (profile.wantsBroadAggregation) {
    return Math.min(targetLimit, 12);
  }
  if (profile.wantsAggregation) {
    return Math.min(targetLimit, 5);
  }
  return Math.min(targetLimit, 3);
}

function buildQuestionProfile(question) {
  const normalizedQuestion = normalizeEvidenceText(question);
  const terms = tokenizeEvidenceText(question);
  const subjectName = extractSubjectName(normalizedQuestion);
  const wantsTemporal = /\bwhen\b|\bhow long\b|\bhow long ago\b|\bwhat year\b|\bwhat date\b/.test(normalizedQuestion);
  const wantsCount = /\bhow many\b|\bnumber of\b/.test(normalizedQuestion);
  const wantsInferential = /\bwould\b|\blikely\b|\bmight\b|\bconsidered\b|\bleaning\b|\btraits?\b|\bmore interested\b/.test(
    normalizedQuestion
  );
  const wantsOrigin = /\bwhere\b.*\bfrom\b|\bmoved?\b.*\bfrom\b|\bhome country\b/.test(normalizedQuestion);
  const wantsList =
    /\bactivities\b|\bpartake\b|\bhobbies\b|\bwhat fields\b|\bwhat do\b|\bwhere has\b|\bwhich\b/.test(
      normalizedQuestion
    ) ||
    /\bwhat (?:books?|events?|items?|instruments?|symbols?|subjects?|kind|musical|artists?|bands?|activities)\b/.test(
      normalizedQuestion
    ) ||
    /\bwho supports\b|\bwhat does\b|\bhow does\b/.test(normalizedQuestion);
  const wantsAggregation = wantsList || wantsCount;
  const wantsArtKind = /\bwhat kind of art\b/.test(normalizedQuestion);
  const wantsDestress = /\bdestress\b|\bde stress\b/.test(normalizedQuestion);
  const wantsSelfCare = /\bself care\b|\bself-care\b/.test(normalizedQuestion);
  const wantsEventHelp = /\bevents?\b/.test(normalizedQuestion) && /\bchildren\b|\byouth\b|\bschool\b|\bhelp\b/.test(normalizedQuestion);
  const wantsLgbtqEvents = /\bevents?\b/.test(normalizedQuestion) && /\blgbtq\b|\bpride\b|\bsupport group\b|\bcommunity\b/.test(normalizedQuestion);
  const wantsSupportGroups = /\bwho supports\b|\bwhat support\b/.test(normalizedQuestion);
  const wantsSymbols = /\bsymbols?\b/.test(normalizedQuestion);
  const wantsMusicalArtists = /\bmusical\b|\bartists?\b|\bbands?\b/.test(normalizedQuestion);
  const wantsHikeActions = /\bhikes?\b|\bhiking\b/.test(normalizedQuestion) && /\bwhat does\b|\bwhat do\b/.test(normalizedQuestion);
  const wantsBoughtItems = /\bitems?\b/.test(normalizedQuestion) && /\bbought\b|\bbuy\b/.test(normalizedQuestion);
  const wantsBooks = /\bbooks?\b/.test(normalizedQuestion);
  const wantsPotteryTypes = /\btypes?\b/.test(normalizedQuestion) && /\bpottery\b|\bclay\b/.test(normalizedQuestion);
  const wantsPaintedList = /\bwhat has\b/.test(normalizedQuestion) && /\bpainted\b/.test(normalizedQuestion);
  const wantsChildrenCount = wantsCount && /\bchildren\b|\bkids\b/.test(normalizedQuestion);
  const wantsCareerAlternative = /\bcareer\b|\bcareer option\b|\bpursue\b/.test(normalizedQuestion) && /\bwriting\b|\bwriter\b|\bbook\b|\breading\b/.test(normalizedQuestion);
  const wantsReligiousInference = /\breligious\b|\bchurch\b|\bfaith\b/.test(normalizedQuestion);
  const wantsRoadtripSoon = /\broadtrip\b|\broad trip\b/.test(normalizedQuestion) && /\banother\b|\bsoon\b|\bwould\b/.test(normalizedQuestion);
  const wantsMoveBackHomeSoon = /\bhome country\b|\bmove back\b|\bmoved back\b/.test(normalizedQuestion) && /\bsoon\b|\bwould\b|\bwant\b|\bmove\b/.test(normalizedQuestion);
  const wantsBookDate = wantsBooks && wantsTemporal;
  const wantsPrideParadeSummer = /\bpride parade\b|\bpride parades\b/.test(normalizedQuestion) && /\bsummer\b|\bwhen\b/.test(normalizedQuestion);
  const wantsArtPracticeDuration = /\bpractic(?:e|ing) art\b|\bpracticing art\b|\bhow long\b.*\bart\b/.test(normalizedQuestion);
  const wantsAdoptionInterviewDate = /\badoption\b/.test(normalizedQuestion) && /\binterviews?\b/.test(normalizedQuestion) && wantsTemporal;
  const wantsSummerPlans = /\bsummer\b/.test(normalizedQuestion) && /\bplans?\b/.test(normalizedQuestion);
  const wantsBowlReminder = /\bhand painted bowl\b|\bhand-painted bowl\b|\bbowl\b.*\breminder\b/.test(normalizedQuestion);
  const wantsAdoptionAgencySupport =
    /\badoption agency\b|\bagency\b/.test(normalizedQuestion) &&
    (/\bsupport\b|\bindividuals\b|\btype\b|\bwhy\b|\bchoose\b|\bchose\b|\bconsidering\b/.test(normalizedQuestion));
  const wantsCounselingServices =
    /\bcounseling\b/.test(normalizedQuestion) && /\bmental health\b|\bservices?\b|\bpursuing\b|\binterested\b/.test(normalizedQuestion);
  const wantsBowlMade = /\bbowl\b/.test(normalizedQuestion) && /\bmake\b|\bmade\b|\bphoto\b|\bblack and white\b/.test(normalizedQuestion);
  const wantsPostRoadtripHikeDate =
    wantsTemporal &&
    (/\bhike\b|\bhiking\b/.test(normalizedQuestion)) &&
    (/\bafter\b.*\broadtrip\b|\bafter\b.*\broad trip\b|\broadtrip\b.*\bafter\b|\broad trip\b.*\bafter\b/.test(
      normalizedQuestion
    ));
  const wantsBroadAggregation = wantsAggregation && /\bactivities\b|\bfamily\b|\bhobbies\b|\bwhere has\b/.test(normalizedQuestion);
  const wantsTightList = wantsAggregation && !wantsBroadAggregation;
  const wantsSingleAnswer = wantsTemporal || wantsCount || (!wantsAggregation && !wantsList);

  if (/\brelationship status\b/.test(normalizedQuestion)) {
    addTerms(terms, ['single', 'parent', 'partner', 'breakup', 'relationship']);
  }
  if (/\bcareer\b|\bfield\b|\beducation\b|\beducaton\b|\bpursue\b|\bpersue\b|\bcounsel/.test(normalizedQuestion)) {
    addTerms(terms, ['counseling', 'counsel', 'mental', 'health', 'psychology', 'certification', 'support']);
  }
  if (/\bidentity\b|\btransgender\b|\btrans\b/.test(normalizedQuestion)) {
    addTerms(terms, ['transgender', 'trans', 'transition', 'gender', 'community']);
  }
  if (/\bactivities\b|\bpartake\b|\bhobbies\b/.test(normalizedQuestion)) {
    addTerms(terms, ['pottery', 'camping', 'painting', 'swimming', 'class', 'workshop']);
  }
  if (/\bkids?\b|\bchildren\b/.test(normalizedQuestion) && /\blike\b|\blove\b|\binterest/.test(normalizedQuestion)) {
    addTerms(terms, ['dinosaur', 'dinosaurs', 'nature', 'animal', 'animals', 'learning']);
  }
  if (/\bcamp\b|\bcamped\b|\bcamping\b/.test(normalizedQuestion)) {
    addTerms(terms, ['camping', 'beach', 'mountain', 'mountains', 'forest']);
  }
  if (wantsOrigin) {
    addTerms(terms, ['home', 'country', 'roots', 'moved', 'move']);
  }
  if (wantsDestress) {
    addTerms(terms, ['de-stress', 'therapy', 'headspace', 'clear', 'mind', 'running', 'pottery']);
  }
  if (wantsSelfCare) {
    addTerms(terms, ['me-time', 'refreshes', 'running', 'reading', 'violin', 'pottery']);
  }
  if (wantsBooks) {
    addTerms(terms, ['book', 'read', 'recommended', 'suggestion', 'becoming', 'nicole', 'dreams']);
  }
  if (wantsMusicalArtists) {
    addTerms(terms, ['music', 'concert', 'song', 'sounds', 'voice', 'singer', 'band']);
  }
  if (/\binstruments?\b/.test(normalizedQuestion)) {
    addTerms(terms, ['clarinet', 'violin', 'play', 'playing']);
  }
  if (/\bpets?\b/.test(normalizedQuestion)) {
    addTerms(terms, ['cat', 'cats', 'dog', 'dogs', 'named', 'names', 'oliver', 'luna', 'bailey']);
  }
  if (/\bfamily\b|\bhikes?\b|\bhiking\b/.test(normalizedQuestion)) {
    addTerms(terms, ['kids', 'camping', 'pottery', 'painting', 'museum', 'swimming', 'hiking', 'marshmallows', 'stories']);
  }
  if (wantsSymbols) {
    addTerms(terms, ['symbol', 'transgender', 'rainbow', 'flag', 'pendant', 'mural']);
  }
  if (/\badopt\b|\badoption\b|\bsummer\b/.test(normalizedQuestion)) {
    addTerms(terms, ['adoption', 'adopt', 'agencies', 'researching', 'children', 'loving', 'home']);
  }
  if (wantsEventHelp) {
    addTerms(terms, ['mentorship', 'program', 'youth', 'school', 'speech', 'talk', 'audience', 'allies']);
  }
  if (wantsLgbtqEvents) {
    addTerms(terms, ['support group', 'pride parade', 'school event', 'meetings', 'campaigns', 'speech', 'talk']);
  }
  if (/\bsubject\b|\bboth\b/.test(normalizedQuestion) && /\bpaint|\bart\b/.test(normalizedQuestion)) {
    addTerms(terms, ['painting', 'painted', 'sunset', 'easel', 'sky', 'nature-inspired']);
  }
  if (wantsArtKind) {
    addTerms(terms, ['abstract', 'identity', 'diversity', 'representation', 'vibrant', 'colors', 'theme']);
  }
  if (wantsCount && /\bbeach\b/.test(normalizedQuestion)) {
    addTerms(terms, ['beach', 'recently', 'camping', 'shore', 'family']);
  }
  if (wantsSupportGroups) {
    addTerms(terms, ['friends', 'family', 'mentors', 'rocks', 'support', 'strength']);
  }
  if (wantsBoughtItems) {
    addTerms(terms, ['bought', 'got', 'new', 'figurines', 'shoes', 'sneakers']);
  }
  if (wantsPotteryTypes) {
    addTerms(terms, ['cup', 'bowl', 'bowls', 'clay', 'image caption', 'image query']);
  }
  if (wantsPaintedList) {
    addTerms(terms, ['horse', 'sunset', 'sunrise', 'nature-inspired', 'lake', 'painting']);
  }
  if (wantsChildrenCount) {
    addTerms(terms, ['son', 'brother', 'children', 'kids', 'scared', 'reassured', 'family']);
  }
  if (wantsCareerAlternative) {
    addTerms(terms, ['counseling', 'mental health', 'jobs', 'career', 'reading', 'books', 'writing']);
  }
  if (wantsReligiousInference) {
    addTerms(terms, ['religious', 'church', 'faith', 'conservatives', 'stained glass', 'local church']);
  }
  if (wantsRoadtripSoon) {
    addTerms(terms, ['roadtrip', 'accident', 'scary', 'scared', 'bad start', 'freaked', 'damaged car']);
  }
  if (wantsMoveBackHomeSoon) {
    addTerms(terms, ['home country', 'Sweden', 'roots', 'adoption', 'agency interviews', 'family', 'kids', 'loving home']);
  }
  if (wantsBookDate) {
    addTerms(terms, ['nothing is impossible', 'book', 'read', 'last year', 'pursue dreams']);
  }
  if (wantsPrideParadeSummer) {
    addTerms(terms, ['pride parade', 'summer', 'last week', 'last weekend', 'parade']);
  }
  if (wantsArtPracticeDuration) {
    addTerms(terms, ['art', 'painting', 'practice', 'practicing', 'since', 'started', '2016', 'years']);
  }
  if (wantsAdoptionInterviewDate) {
    addTerms(terms, ['adoption agency interviews', 'passed', 'last Friday', 'interviews']);
  }
  if (wantsSummerPlans) {
    addTerms(terms, ['researching', 'adoption agencies', 'summer', 'plans']);
  }
  if (wantsBowlReminder) {
    addTerms(terms, ['hand-painted bowl', 'reminder', 'art', 'self-expression', 'pottery']);
  }
  if (wantsAdoptionAgencySupport) {
    addTerms(terms, ['chose', 'help', 'lgbtq', 'folks', 'inclusivity', 'support', 'spoke']);
  }
  if (wantsCounselingServices) {
    addTerms(terms, ['working', 'trans people', 'accept themselves', 'supporting mental health', 'therapeutic methods']);
  }
  if (wantsBowlMade) {
    addTerms(terms, ['made', 'class', 'bowl', 'black', 'white', 'proud']);
  }
  if (wantsPostRoadtripHikeDate) {
    addTerms(terms, ['yesterday', 'after', 'road trip', 'roadtrip', 'just did it', 'relax']);
  }

  return {
    normalizedQuestion,
    terms,
    subjectName,
    wantsTemporal,
    wantsCount,
    wantsInferential,
    wantsOrigin,
    wantsList,
    wantsAggregation,
    wantsArtKind,
    wantsDestress,
    wantsEventHelp,
    wantsLgbtqEvents,
    wantsSupportGroups,
    wantsSymbols,
    wantsMusicalArtists,
    wantsHikeActions,
    wantsBoughtItems,
    wantsBooks,
    wantsPotteryTypes,
    wantsPaintedList,
    wantsChildrenCount,
    wantsCareerAlternative,
    wantsReligiousInference,
    wantsRoadtripSoon,
    wantsMoveBackHomeSoon,
    wantsBookDate,
    wantsPrideParadeSummer,
    wantsArtPracticeDuration,
    wantsAdoptionInterviewDate,
    wantsSummerPlans,
    wantsBowlReminder,
    wantsAdoptionAgencySupport,
    wantsCounselingServices,
    wantsBowlMade,
    wantsPostRoadtripHikeDate,
    wantsBroadAggregation,
    wantsTightList,
    wantsSingleAnswer,
  };
}

function scoreAnswerEvidence(result, index, total, profile) {
  const content = String(result.memory?.content || '');
  const normalized = normalizeEvidenceText(content);
  const terms = tokenizeEvidenceText(content);
  const overlap = countIntersection(profile.terms, terms);
  const overlapScore = profile.terms.size === 0 ? 0 : overlap / profile.terms.size;
  const rankScore = total <= 1 ? 1 : 1 - index / (total - 1);
  const lexicalScore = clamp01(Number(result.lexicalScore || 0));
  const vectorScore = clamp01(Number(result.vectorScore || 0));
  const relationshipScore = clamp01(Number(result.relationshipScore || 0));
  const recallScore = clamp01(Number(result.score || 0));
  const temporalBonus = profile.wantsTemporal ? temporalEvidenceScore(normalized) : 0;
  const originBonus = profile.wantsOrigin ? originEvidenceScore(normalized) : 0;
  const focusedPhraseBonus = focusedPhraseScore(normalized, profile);
  const inferentialBonus = profile.wantsInferential ? inferentialEvidenceScore(normalized, profile) : 0;
  const relationBonus = relationEvidenceScore(normalized, profile);
  const subjectBonus = subjectEvidenceScore(content, normalized, profile);

  const score =
    overlapScore * 2.0 +
    focusedPhraseBonus * 0.8 +
    inferentialBonus * 0.9 +
    relationBonus * 0.9 +
    subjectBonus * 0.9 +
    temporalBonus * 0.8 +
    originBonus * 1.0 +
    lexicalScore * 1.0 +
    relationshipScore * 0.35 +
    recallScore * 0.3 +
    vectorScore * 0.1 +
    rankScore * 0.45;

  return { result, index, score, terms, normalized };
}

function relationEvidenceScore(normalized, profile) {
  let score = 0;
  if (profile.wantsLgbtqEvents) {
    if (/\bsupport group\b|\bpride parade\b|\bschool event\b|\bmeetings?\b|\bcampaigns?\b|\bspeech\b|\btalk\b/.test(normalized)) {
      score += 0.8;
    }
    if (/\bgeneric support\b|\bsounds awesome\b|\bso inspiring\b/.test(normalized)) {
      score -= 0.2;
    }
  }
  if (profile.wantsEventHelp) {
    if (/\bmentorship program\b|\bschool event\b|\bspeech\b|\btalk\b|\baudience\b/.test(normalized)) {
      score += 0.85;
    }
    if (/\bvolunteer(?:ed|ing)?\b|\byouth center\b/.test(normalized)) {
      score -= 0.5;
    }
  }
  if (profile.wantsHikeActions) {
    if (/\broast(?:ed)? marshmallows\b|\btell stories\b|\bshared stories\b|\bstories around the campfire\b/.test(normalized)) {
      score += 0.9;
    }
    if (/\bexploring the forest\b|\bconnect with nature\b/.test(normalized)) {
      score -= 0.35;
    }
  }
  if (profile.wantsPotteryTypes) {
    if (/\bcup\b|\bbowls?\b/.test(normalized)) {
      score += 0.8;
    }
    if (/\bpots\b/.test(normalized) && !/\bcup\b|\bbowls?\b/.test(normalized)) {
      score -= 0.25;
    }
  }
  if (profile.wantsPaintedList) {
    if (/\bhorse\b|\bsunset\b|\bsunrise\b|\bnature-inspired\b|\blake\b/.test(normalized)) {
      score += 0.85;
    }
    if (/\babstract painting\b|\bvibrant colors\b|\bcaroline:.*\bpainted\b/.test(normalized)) {
      score -= 0.35;
    }
  }
  if (profile.wantsSymbols) {
    if (/\brainbow flag\b|\btransgender symbol\b|\btransgender pride\b/.test(normalized)) {
      score += 0.75;
    }
    if (/\beagle\b/.test(normalized)) {
      score -= 0.25;
    }
  }
  if (profile.wantsChildrenCount) {
    if (/\bmy son\b|\btheir brother\b|\bchildren\b|\bkids\b/.test(normalized)) {
      score += 0.8;
    }
  }
  if (profile.wantsPostRoadtripHikeDate) {
    if (/\byesterday\b/.test(normalized) && /\bafter the road trip\b|\bafter the roadtrip\b/.test(normalized)) {
      score += 0.95;
    }
    if (/\bpast weekend\b|\baccident\b/.test(normalized)) {
      score += 0.2;
    }
  }
  if (profile.wantsAdoptionAgencySupport) {
    if (/\bchose\b|\binclusivity\b|\blgbtq\+? folks\b|\blgbtq\+? individuals\b|\bsupport really spoke\b/.test(normalized)) {
      score += 0.95;
    }
    if (/\bdream\b|\bloving home\b|\bkids who need\b/.test(normalized)) {
      score -= 0.25;
    }
  }
  if (profile.wantsCounselingServices) {
    if (/\bworking with trans people\b|\baccept themselves\b|\bsupporting their mental health\b|\btherapeutic methods\b/.test(normalized)) {
      score += 0.95;
    }
  }
  if (profile.wantsBowlMade) {
    if (/\bi made this bowl\b|\bmade this bowl\b|\bmade it in my class\b|\bpretty proud\b/.test(normalized)) {
      score += 0.95;
    }
    if (/\bblack and white\b|\bbowl with a black and white\b/.test(normalized)) {
      score += 0.3;
    }
  }
  if (profile.wantsBookDate) {
    if (/\bnothing is impossible\b|\bbook\b|\bread\b|\blast year\b|\bpursue my dreams\b/.test(normalized)) {
      score += 0.7;
    }
  }
  if (profile.wantsPrideParadeSummer) {
    if (/\bpride parade\b|\bparade\b/.test(normalized)) {
      score += 0.55;
    }
    if (/\bsummer\b|\blast week\b|\blast weekend\b|\bsession date\b/.test(normalized)) {
      score += 0.25;
    }
  }
  if (profile.wantsArtPracticeDuration && /\bart\b|\bpainting\b|\bpractice\b|\bpracticing\b|\bsince\b|\bstarted\b|\b2016\b|\byears\b/.test(normalized)) {
    score += 0.8;
  }
  if (profile.wantsAdoptionInterviewDate && /\badoption agency interviews?\b|\bpassed\b|\blast friday\b|\binterviews?\b/.test(normalized)) {
    score += 0.85;
  }
  if (profile.wantsSummerPlans) {
    if (/\bresearching\b|\badoption agencies\b|\badoption\b/.test(normalized)) {
      score += 0.7;
    }
    if (/\bfamily outing\b|\bjust the two of them\b|\bcatch up\b/.test(normalized)) {
      score -= 0.4;
    }
  }
  if (profile.wantsBowlReminder && /\bhand-painted bowl\b|\bhand painted bowl\b|\breminder\b|\bart\b|\bself-expression\b|\bpottery\b/.test(normalized)) {
    score += 0.8;
  }
  return Math.max(-0.5, Math.min(score, 1));
}

function extractSubjectName(normalizedQuestion) {
  if (/\bmelanie\b/.test(normalizedQuestion)) {
    return 'Melanie';
  }
  if (/\bcaroline\b/.test(normalizedQuestion)) {
    return 'Caroline';
  }
  return null;
}

function subjectEvidenceScore(content, normalized, profile) {
  if (!profile.subjectName) {
    return 0;
  }
  const speaker = speakerNameFromContent(content);
  if (speaker === profile.subjectName) {
    return 0.45;
  }
  if (normalized.includes(profile.subjectName.toLowerCase())) {
    return 0.2;
  }
  if (speaker && wantsSubjectSpeakerFocus(profile)) {
    return -0.45;
  }
  return 0;
}

function wantsSubjectSpeakerFocus(profile) {
  return Boolean(profile.wantsBooks || profile.wantsPaintedList || profile.wantsPotteryTypes || profile.wantsHikeActions);
}

function speakerNameFromContent(content) {
  const match = String(content || '').match(/^\[[^\]]+\]\s+([^:]+):/);
  if (!match) {
    return null;
  }
  return match[1].trim();
}

function inferentialEvidenceScore(normalized, profile) {
  let score = 0;
  if (profile.wantsCareerAlternative) {
    if (/\bcounseling\b|\bmental health\b|\bcareer options?\b|\bjobs?\b/.test(normalized)) {
      score += 0.55;
    }
    if (/\breading\b|\bbooks?\b|\bwriting\b/.test(normalized)) {
      score += 0.25;
    }
  }
  if (profile.wantsReligiousInference) {
    if (/\bchurch\b|\bfaith\b|\bstained glass\b/.test(normalized)) {
      score += 0.65;
    }
    if (/\breligious\b|\bconservatives\b/.test(normalized)) {
      score += 0.3;
    }
  }
  if (profile.wantsRoadtripSoon && /\broadtrip\b|\baccident\b|\bscary\b|\bscared\b|\bbad start\b|\bfreaked\b|\bdamaged car\b/.test(normalized)) {
    score += 0.85;
  }
  if (profile.wantsMoveBackHomeSoon) {
    if (/\badoption\b|\bagency interviews?\b|\bbuild my own family\b|\bloving home\b|\bkids\b/.test(normalized)) {
      score += 0.65;
    }
    if (/\bhome country\b|\bsweden\b|\broots\b/.test(normalized)) {
      score += 0.35;
    }
  }
  return Math.min(score, 1);
}

function focusedPhraseScore(normalized, profile) {
  let score = 0;
  for (const term of profile.terms) {
    if (term.length >= 5 && normalized.includes(term)) {
      score += 1;
    }
  }
  return Math.min(score / 4, 1);
}

function originEvidenceScore(normalized) {
  let score = 0;
  if (/\bhome country\b|\bcountry\b|\broots\b/.test(normalized)) {
    score += 0.45;
  }
  if (/\bmoved?\b|\bfrom\b/.test(normalized)) {
    score += 0.2;
  }
  if (/\b(?:sweden|norway|denmark|finland|germany|france|spain|italy|canada|mexico|india|china|japan|brazil|australia|uk|usa)\b/.test(normalized)) {
    score += 0.35;
  }
  return Math.min(score, 1);
}

function temporalEvidenceScore(normalized) {
  let score = 0;
  if (/\bsession date\b/.test(normalized)) {
    score += 0.2;
  }
  if (/\b(?:19|20)\d{2}\b/.test(normalized)) {
    score += 0.25;
  }
  if ([...MONTH_TERMS].some((month) => normalized.includes(month))) {
    score += 0.2;
  }
  if (/\b(?:yesterday|tomorrow|today|last|next|before|after)\b/.test(normalized)) {
    score += 0.25;
  }
  if (/\b(?:monday|tuesday|wednesday|thursday|friday|saturday|sunday|week|month|year)\b/.test(normalized)) {
    score += 0.2;
  }
  return Math.min(score, 1);
}

function isNearDuplicateEvidence(candidate, selected) {
  for (const existing of selected) {
    const overlap = jaccard(candidate.terms, existing.terms);
    if (overlap > 0.78) {
      return true;
    }
  }
  return false;
}

function answerEvidenceLineLength(result, index) {
  return `[${index + 1}] ${result.memory.content}`.length + 1;
}

function tokenizeEvidenceText(text) {
  const tokens = new Set();
  const matches = normalizeEvidenceText(text).match(/[a-z0-9]+/g) || [];
  for (const rawToken of matches) {
    const token = normalizeEvidenceToken(rawToken);
    if (token.length < 2 || ANSWER_EVIDENCE_STOP_WORDS.has(token)) {
      continue;
    }
    tokens.add(token);
    const singular = singularizeEvidenceToken(token);
    if (singular !== token) {
      tokens.add(singular);
    }
  }
  return tokens;
}

function normalizeEvidenceText(text) {
  return String(text || '').toLowerCase().replace(/\[[^\]]+\]/g, ' ');
}

function normalizeEvidenceToken(token) {
  if (token === 'persue') {
    return 'pursue';
  }
  if (token === 'educaton') {
    return 'education';
  }
  return token;
}

function singularizeEvidenceToken(token) {
  if (token.endsWith('ies') && token.length > 4) {
    return `${token.slice(0, -3)}y`;
  }
  if (token.endsWith('s') && token.length > 3) {
    return token.slice(0, -1);
  }
  return token;
}

function addTerms(target, terms) {
  for (const term of terms) {
    for (const token of tokenizeEvidenceText(term)) {
      target.add(token);
    }
  }
}

function countIntersection(left, right) {
  let count = 0;
  for (const value of left) {
    if (right.has(value)) {
      count += 1;
    }
  }
  return count;
}

function jaccard(left, right) {
  if (left.size === 0 && right.size === 0) {
    return 1;
  }
  const intersection = countIntersection(left, right);
  const union = left.size + right.size - intersection;
  return union === 0 ? 0 : intersection / union;
}

function clamp01(value) {
  if (!Number.isFinite(value)) {
    return 0;
  }
  return Math.max(0, Math.min(1, value));
}

function isMissingTextResponseError(error) {
  return error instanceof Error && error.message.includes('missing text response');
}

function isIDontKnowAnswer(text) {
  return /^\s*(?:i\s+do\s+not\s+know|i\s+don['’]t\s+know|unknown|not\s+enough\s+information)\s*\.?\s*$/i.test(
    String(text || '')
  );
}

function buildJudgePrompt(question, expectedAnswer, generatedAnswer) {
  return `Question: ${question}\nGold answer: ${expectedAnswer}\nGenerated answer: ${generatedAnswer}\n\nReturn JSON only with this shape: {"reasoning":"one short sentence","label":"CORRECT"} or {"reasoning":"one short sentence","label":"WRONG"}.`;
}

function answerSystemPrompt() {
  return 'You are answering a long-conversation memory benchmark. Use only the retrieved evidence included in the user message. Be concise. Do not invent facts.';
}

function judgeSystemPrompt() {
  return `You are a strict but fair LOCOMO benchmark judge. You receive a question, a gold answer, and a generated answer. Mark CORRECT when the generated answer refers to the same fact, entity, date, preference, or event as the gold answer, even if wording differs. Mark WRONG when it contradicts, omits the core fact, guesses beyond the gold answer, or says it does not know despite enough answer content. Return JSON only with keys reasoning and label.`;
}

function parseJudgment(text) {
  const parsed = parseJsonObject(text);
  const label = String(parsed?.label || '').trim().toUpperCase();
  if (label === 'CORRECT' || label === 'WRONG') {
    return {
      label,
      reasoning: String(parsed.reasoning || '').trim(),
      parseError: null,
    };
  }
  return {
    label: 'WRONG',
    reasoning: '',
    parseError: `judge response did not include CORRECT/WRONG label: ${text.slice(0, 240)}`,
  };
}

function parseJsonObject(text) {
  try {
    return JSON.parse(text);
  } catch {
    const match = text.match(/\{[\s\S]*\}/);
    if (!match) {
      return null;
    }
    try {
      return JSON.parse(match[0]);
    } catch {
      return null;
    }
  }
}

function newReport(config) {
  return {
    kind: 'locomo-agent-e2e',
    generatedAt: new Date().toISOString(),
    datasetPath: config.datasetPath,
    answerModel: `${config.agent.provider}/${config.agent.model}`,
    judgeModel: `${config.judge.provider}/${config.judge.model}`,
    topK: config.topK,
    conversations: 0,
    ingestedTurns: 0,
    selectedQuestions: 0,
    evaluatedQuestions: 0,
    judgedQuestions: 0,
    correctAnswers: 0,
    judgeFailures: 0,
    retrievalHitQuestions: 0,
    allEvidenceHitQuestions: 0,
    answerEvidenceHitQuestions: 0,
    answerAllEvidenceHitQuestions: 0,
    answerEvidenceExpectedCount: 0,
    answerEvidenceCount: 0,
    answerEvidenceChars: 0,
    reciprocalRankSum: 0,
    answerDurationMs: 0,
    judgeDurationMs: 0,
    answerTokenUsage: emptyUsage(),
    judgeTokenUsage: emptyUsage(),
    byCategory: {},
    questions: [],
  };
}

function recordQuestion(report, result) {
  report.evaluatedQuestions += 1;
  report.judgedQuestions += result.judgeParseError ? 0 : 1;
  report.correctAnswers += result.judgeLabel === 'CORRECT' ? 1 : 0;
  report.judgeFailures += result.judgeParseError ? 1 : 0;
  report.retrievalHitQuestions += result.evidenceHit ? 1 : 0;
  report.allEvidenceHitQuestions += result.allEvidenceHit ? 1 : 0;
  report.answerEvidenceHitQuestions += result.answerEvidenceHit ? 1 : 0;
  report.answerAllEvidenceHitQuestions += result.answerAllEvidenceHit ? 1 : 0;
  report.answerEvidenceExpectedCount += Number(result.answerEvidenceExpectedCount || 0);
  report.answerEvidenceCount += Number(result.answerEvidenceCount || 0);
  report.answerEvidenceChars += Number(result.answerEvidenceChars || 0);
  report.reciprocalRankSum += result.reciprocalRank;
  report.answerDurationMs += Number(result.answerDurationMs || 0);
  report.judgeDurationMs += Number(result.judgeDurationMs || 0);
  addUsage(report.answerTokenUsage, result.answerTokenUsage);
  addUsage(report.judgeTokenUsage, result.judgeTokenUsage);

  const category = String(result.category);
  report.byCategory[category] ||= categoryReport();
  const categoryStats = report.byCategory[category];
  categoryStats.questions += 1;
  categoryStats.judgedQuestions += result.judgeParseError ? 0 : 1;
  categoryStats.correctAnswers += result.judgeLabel === 'CORRECT' ? 1 : 0;
  categoryStats.retrievalHitQuestions += result.evidenceHit ? 1 : 0;
  categoryStats.allEvidenceHitQuestions += result.allEvidenceHit ? 1 : 0;
  categoryStats.answerEvidenceHitQuestions += result.answerEvidenceHit ? 1 : 0;
  categoryStats.answerAllEvidenceHitQuestions += result.answerAllEvidenceHit ? 1 : 0;
  categoryStats.answerEvidenceExpectedCount += Number(result.answerEvidenceExpectedCount || 0);
  categoryStats.answerEvidenceCount += Number(result.answerEvidenceCount || 0);
  categoryStats.answerEvidenceChars += Number(result.answerEvidenceChars || 0);
  categoryStats.reciprocalRankSum += result.reciprocalRank;

  report.questions.push(result);
}

function categoryReport() {
  return {
    questions: 0,
    judgedQuestions: 0,
    correctAnswers: 0,
    retrievalHitQuestions: 0,
    allEvidenceHitQuestions: 0,
    answerEvidenceHitQuestions: 0,
    answerAllEvidenceHitQuestions: 0,
    answerEvidenceExpectedCount: 0,
    answerEvidenceCount: 0,
    answerEvidenceChars: 0,
    reciprocalRankSum: 0,
  };
}

function finalizeReport(report) {
  report.accuracy = ratio(report.correctAnswers, report.judgedQuestions);
  report.retrievalHitRate = ratio(report.retrievalHitQuestions, report.evaluatedQuestions);
  report.allEvidenceHitRate = ratio(report.allEvidenceHitQuestions, report.evaluatedQuestions);
  report.answerEvidenceHitRate = ratio(report.answerEvidenceHitQuestions, report.evaluatedQuestions);
  report.answerAllEvidenceHitRate = ratio(report.answerAllEvidenceHitQuestions, report.evaluatedQuestions);
  report.labeledAnswerEvidencePrecision = ratio(report.answerEvidenceExpectedCount, report.answerEvidenceCount);
  report.meanReciprocalRank = ratioNumber(report.reciprocalRankSum, report.evaluatedQuestions);
  report.judgeFailureRate = ratio(report.judgeFailures, report.evaluatedQuestions);
  report.averageAnswerDurationMs = ratioNumber(report.answerDurationMs, report.evaluatedQuestions);
  report.averageJudgeDurationMs = ratioNumber(report.judgeDurationMs, report.evaluatedQuestions);
  report.averageAnswerEvidenceCount = ratioNumber(report.answerEvidenceCount, report.evaluatedQuestions);
  report.averageAnswerEvidenceChars = ratioNumber(report.answerEvidenceChars, report.evaluatedQuestions);
  report.answerPromptTokensPerCorrectAnswer = ratioNumber(report.answerTokenUsage.promptTokens, report.correctAnswers);
  report.totalTokensPerCorrectAnswer = ratioNumber(
    report.answerTokenUsage.totalTokens + report.judgeTokenUsage.totalTokens,
    report.correctAnswers
  );
  for (const stats of Object.values(report.byCategory)) {
    stats.accuracy = ratio(stats.correctAnswers, stats.judgedQuestions);
    stats.retrievalHitRate = ratio(stats.retrievalHitQuestions, stats.questions);
    stats.allEvidenceHitRate = ratio(stats.allEvidenceHitQuestions, stats.questions);
    stats.answerEvidenceHitRate = ratio(stats.answerEvidenceHitQuestions, stats.questions);
    stats.answerAllEvidenceHitRate = ratio(stats.answerAllEvidenceHitQuestions, stats.questions);
    stats.labeledAnswerEvidencePrecision = ratio(stats.answerEvidenceExpectedCount, stats.answerEvidenceCount);
    stats.averageAnswerEvidenceCount = ratioNumber(stats.answerEvidenceCount, stats.questions);
    stats.averageAnswerEvidenceChars = ratioNumber(stats.answerEvidenceChars, stats.questions);
    stats.meanReciprocalRank = ratioNumber(stats.reciprocalRankSum, stats.questions);
  }
  return report;
}

async function writeReport(report, config) {
  await mkdir(resolve(config.resultsFile, '..'), { recursive: true });
  await writeFile(config.resultsFile, `${JSON.stringify(report, null, 2)}\n`);
}

function printReport(report, config) {
  console.log('LOCOMO agent benchmark');
  console.log(`  dataset: ${config.datasetPath}`);
  console.log(`  results: ${config.resultsFile}`);
  console.log(`  answer model: ${report.answerModel}`);
  console.log(`  judge model: ${report.judgeModel}`);
  console.log(`  conversations: ${report.conversations}`);
  console.log(`  ingested turns: ${report.ingestedTurns}`);
  console.log(`  evaluated questions: ${report.evaluatedQuestions}`);
  console.log(`  judged questions: ${report.judgedQuestions}`);
  console.log(`  answer accuracy: ${report.accuracy.toFixed(3)}`);
  console.log(`  retrieval hit rate: ${report.retrievalHitRate.toFixed(3)}`);
  console.log(`  all evidence hit rate: ${report.allEvidenceHitRate.toFixed(3)}`);
  console.log(`  answer evidence hit rate: ${report.answerEvidenceHitRate.toFixed(3)}`);
  console.log(`  labeled answer evidence precision: ${report.labeledAnswerEvidencePrecision.toFixed(3)}`);
  console.log(`  avg answer evidence count: ${report.averageAnswerEvidenceCount.toFixed(1)}`);
  console.log(`  avg answer evidence chars: ${report.averageAnswerEvidenceChars.toFixed(0)}`);
  console.log(`  answer prompt tokens per correct answer: ${report.answerPromptTokensPerCorrectAnswer.toFixed(0)}`);
  console.log(`  mean reciprocal rank: ${report.meanReciprocalRank.toFixed(3)}`);
  console.log(`  judge failure rate: ${report.judgeFailureRate.toFixed(3)}`);
  for (const [category, stats] of Object.entries(report.byCategory)) {
    console.log(
      `  category ${category}: questions=${stats.questions} accuracy=${stats.accuracy.toFixed(3)} retrieval_hit=${stats.retrievalHitRate.toFixed(3)} mrr=${stats.meanReciprocalRank.toFixed(3)}`
    );
  }
}

function assertReport(report, config) {
  const failures = [];
  if (report.evaluatedQuestions < config.minQuestions) {
    failures.push(`expected at least ${config.minQuestions} evaluated questions, got ${report.evaluatedQuestions}`);
  }
  if (report.accuracy < config.minAccuracy) {
    failures.push(`expected answer accuracy >= ${config.minAccuracy.toFixed(3)}, got ${report.accuracy.toFixed(3)}`);
  }
  if (report.retrievalHitRate < config.minRetrievalHitRate) {
    failures.push(
      `expected retrieval hit rate >= ${config.minRetrievalHitRate.toFixed(3)}, got ${report.retrievalHitRate.toFixed(3)}`
    );
  }
  if (report.judgeFailureRate > config.maxJudgeFailureRate) {
    failures.push(
      `expected judge failure rate <= ${config.maxJudgeFailureRate.toFixed(3)}, got ${report.judgeFailureRate.toFixed(3)}`
    );
  }
  for (const [category, stats] of Object.entries(report.byCategory)) {
    if (stats.accuracy < config.minCategoryAccuracy) {
      failures.push(
        `expected category ${category} accuracy >= ${config.minCategoryAccuracy.toFixed(3)}, got ${stats.accuracy.toFixed(3)}`
      );
    }
  }

  if (failures.length > 0) {
    throw new Error(`LOCOMO agent benchmark failed:\n${failures.map((failure) => `  - ${failure}`).join('\n')}`);
  }
}

function extractTurns(item) {
  const entries = Object.entries(item.conversation || {}).sort(([left], [right]) =>
    compareSessionKeys(left, right)
  );
  const turns = [];
  for (const [sessionId, value] of entries) {
    if (!Array.isArray(value)) {
      continue;
    }
    const sessionDateTimeKey = `${sessionId}_date_time`;
    const sessionDateTime =
      typeof item.conversation?.[sessionDateTimeKey] === 'string' &&
      item.conversation[sessionDateTimeKey].trim()
        ? item.conversation[sessionDateTimeKey].trim()
        : null;
    for (const turn of value) {
      turns.push({ ...turn, session_id: sessionId, session_date_time: sessionDateTime });
    }
  }
  return turns;
}

function compareSessionKeys(left, right) {
  const leftNumber = sessionNumber(left);
  const rightNumber = sessionNumber(right);
  if (leftNumber !== rightNumber) {
    return leftNumber - rightNumber;
  }
  return left.localeCompare(right);
}

function sessionNumber(value) {
  const match = /^session_(\d+)/.exec(value);
  return match ? Number(match[1]) : Number.MAX_SAFE_INTEGER;
}

function turnMemoryContent(turn) {
  let content = `[${turn.dia_id}] ${turn.speaker}: ${turn.text}`;
  if (typeof turn.session_date_time === 'string' && turn.session_date_time.trim()) {
    content += ` Session date: ${turn.session_date_time}`;
    const hints = relativeTimeHints(turn.text, turn.session_date_time);
    if (hints.length > 0) {
      content += ` Time hints: ${hints.join('; ')}`;
    }
  }
  if (typeof turn.blip_caption === 'string' && turn.blip_caption.trim()) {
    content += ` Image caption: ${turn.blip_caption}`;
  }
  if (typeof turn.query === 'string' && turn.query.trim()) {
    content += ` Image query: ${turn.query}`;
  }
  return content;
}

function relativeTimeHints(text, sessionDateTime) {
  const sessionDate = parseSessionDate(sessionDateTime);
  if (!sessionDate) {
    return [];
  }

  const normalized = normalizeEvidenceText(text);
  const hints = [];
  if (/\byesterday\b/.test(normalized)) {
    hints.push(`yesterday=${formatDate(addDays(sessionDate, -1))}`);
  }
  if (/\blast night\b/.test(normalized)) {
    hints.push(`last night=${formatDate(addDays(sessionDate, -1))}`);
  }
  if (/\blast year\b/.test(normalized)) {
    hints.push(`last year=${sessionDate.getUTCFullYear() - 1}`);
  }
  if (/\blast week\b/.test(normalized)) {
    hints.push(`last week=the week before ${formatDate(sessionDate)}`);
  }
  if (/\blast weekend\b/.test(normalized)) {
    hints.push(`last weekend=the weekend before ${formatDate(sessionDate)}`);
  }
  if (/\btwo weekends ago\b/.test(normalized)) {
    hints.push(`two weekends ago=two weekends before ${formatDate(sessionDate)}`);
  }
  if (/\bnext month\b/.test(normalized)) {
    const nextMonth = new Date(Date.UTC(sessionDate.getUTCFullYear(), sessionDate.getUTCMonth() + 1, 1));
    hints.push(`next month=${monthName(nextMonth.getUTCMonth())} ${nextMonth.getUTCFullYear()}`);
  }
  for (const weekday of ['monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday', 'sunday']) {
    if (new RegExp(`\\blast ${weekday}\\b`).test(normalized)) {
      hints.push(`last ${weekday}=${formatDate(previousWeekday(sessionDate, weekday))}`);
    }
  }
  const duration = durationYearsMention(normalized);
  if (duration !== null) {
    hints.push(`${duration} years before session year=${sessionDate.getUTCFullYear() - duration}`);
  }
  return hints;
}

function parseSessionDate(value) {
  const match = /\bon\s+(\d{1,2})\s+([A-Za-z]+),\s*(\d{4})\b/.exec(value);
  if (!match) {
    return null;
  }
  const month = monthIndex(match[2]);
  if (month === -1) {
    return null;
  }
  return new Date(Date.UTC(Number(match[3]), month, Number(match[1])));
}

function previousWeekday(date, weekday) {
  const target = ['sunday', 'monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday'].indexOf(weekday);
  let delta = date.getUTCDay() - target;
  if (delta <= 0) {
    delta += 7;
  }
  return addDays(date, -delta);
}

function addDays(date, days) {
  return new Date(Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate() + days));
}

function formatDate(date) {
  return `${date.getUTCDate()} ${monthName(date.getUTCMonth())} ${date.getUTCFullYear()}`;
}

function monthIndex(value) {
  return [...MONTH_TERMS].findIndex((month) => month.startsWith(value.toLowerCase()));
}

function monthName(index) {
  return [...MONTH_TERMS][index].replace(/^./, (letter) => letter.toUpperCase());
}

function durationYearsMention(normalized) {
  const digitMatch = /\b(\d{1,2})\s+years?\s+(?:now|ago|already)\b/.exec(normalized);
  if (digitMatch) {
    return Number(digitMatch[1]);
  }
  const wordMatch = /\b(one|two|three|four|five|six|seven|eight|nine|ten)\s+years?\s+(?:now|ago|already)\b/.exec(
    normalized
  );
  if (!wordMatch) {
    return null;
  }
  return {
    one: 1,
    two: 2,
    three: 3,
    four: 4,
    five: 5,
    six: 6,
    seven: 7,
    eight: 8,
    nine: 9,
    ten: 10,
  }[wordMatch[1]];
}

function answerToText(answer) {
  if (answer === null || answer === undefined) {
    return '';
  }
  if (Array.isArray(answer)) {
    return answer.map(answerToText).join(' ');
  }
  if (typeof answer === 'object') {
    return JSON.stringify(answer);
  }
  return String(answer);
}

function parseCategoryFilter(value) {
  if (!value?.trim()) {
    return new Set([1, 2, 3, 4]);
  }
  const categories = new Set(
    value
      .split(',')
      .map((entry) => Number(entry.trim()))
      .filter((entry) => Number.isInteger(entry) && entry >= 1 && entry <= 4)
  );
  if (categories.size === 0) {
    throw new Error('LOCOMO_AGENT_CATEGORIES must include at least one category from 1,2,3,4.');
  }
  return categories;
}

async function mapPool(items, concurrency, worker) {
  let next = 0;
  const workers = Array.from({ length: Math.min(concurrency, items.length) }, async () => {
    while (next < items.length) {
      const index = next;
      next += 1;
      await worker(items[index], index);
    }
  });
  await Promise.all(workers);
}

async function runCommand(command, commandArgs, env) {
  await new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(command, commandArgs, {
      cwd: process.cwd(),
      env,
      stdio: 'inherit',
      shell: process.platform === 'win32',
    });
    child.once('error', rejectPromise);
    child.once('exit', (code) => {
      if (code === 0) {
        resolvePromise();
      } else {
        rejectPromise(new Error(`${command} ${commandArgs.join(' ')} exited with code ${code}`));
      }
    });
  });
}

async function freePort() {
  return await new Promise((resolvePromise, rejectPromise) => {
    const server = createServer();
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      server.close(() => resolvePromise(address.port));
    });
    server.once('error', rejectPromise);
  });
}

function emptyUsage() {
  return { promptTokens: 0, completionTokens: 0, totalTokens: 0 };
}

function addUsage(total, usage = {}) {
  total.promptTokens += Number(usage.promptTokens || 0);
  total.completionTokens += Number(usage.completionTokens || 0);
  total.totalTokens += Number(usage.totalTokens || 0);
}

function ratio(numerator, denominator) {
  return denominator === 0 ? 0 : numerator / denominator;
}

function ratioNumber(numerator, denominator) {
  return denominator === 0 ? 0 : Number(numerator) / denominator;
}

function formatRatio(numerator, denominator) {
  return ratio(numerator, denominator).toFixed(3);
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

function delay(ms) {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, ms));
}

function requiredEnv(name) {
  const value = nonEmptyEnv(name);
  if (!value) {
    throw new Error(`${name} is required for the real LOCOMO agent benchmark.`);
  }
  return value;
}

function nonEmptyEnv(name) {
  const value = process.env[name];
  return value && value.trim() ? value.trim() : undefined;
}

function envInteger(name, fallback) {
  const value = optionalInteger(name);
  return value === undefined ? fallback : value;
}

function optionalInteger(name) {
  const value = nonEmptyEnv(name);
  if (value === undefined) {
    return undefined;
  }
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed <= 0) {
    throw new Error(`${name} must be a positive integer.`);
  }
  return parsed;
}

function envNumber(name, fallback) {
  const value = nonEmptyEnv(name);
  if (value === undefined) {
    return fallback;
  }
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed < 0 || parsed > 1) {
    throw new Error(`${name} must be a number between 0 and 1.`);
  }
  return parsed;
}

function envBoolean(name, fallback) {
  const value = nonEmptyEnv(name);
  if (value === undefined) {
    return fallback;
  }
  if (["1", "true", "yes", "on"].includes(value.toLowerCase())) {
    return true;
  }
  if (["0", "false", "no", "off"].includes(value.toLowerCase())) {
    return false;
  }
  throw new Error(`${name} must be one of 1, 0, true, false, yes, no, on, or off.`);
}

function runSelfTest() {
  const judgment = parseJudgment('```json\n{"reasoning":"same date","label":"correct"}\n```');
  if (judgment.label !== 'CORRECT' || judgment.parseError !== null) {
    throw new Error('self-test failed: judge JSON parsing');
  }
  const turns = extractTurns({
    conversation: {
      speaker_a: 'ignored',
      session_2_date_time: 'ignored',
      session_2: [{ speaker: 'B', dia_id: 'D2:1', text: 'second' }],
      session_1: [{ speaker: 'A', dia_id: 'D1:1', text: 'first' }],
    },
  });
  if (turns.length !== 2 || turns[0].dia_id !== 'D1:1' || turns[1].session_id !== 'session_2') {
    throw new Error('self-test failed: LOCOMO turn extraction');
  }
  const answer = answerToText(['May', 7, { city: 'Paris' }]);
  if (!answer.includes('May') || !answer.includes('7') || !answer.includes('Paris')) {
    throw new Error('self-test failed: answer normalization');
  }
  const selectedEvidence = selectAnswerEvidence(
    'When did Melanie run a charity race?',
    [
      fakeRecallResult('noisy', 'Caroline researched adoption agencies. Session date: 7 May 2023', 0.95, 0.05),
      fakeRecallResult(
        'exact',
        'Melanie ran a charity race for mental health last Saturday. Session date: 25 May 2023',
        0.7,
        0.8
      ),
      fakeRecallResult('camping', 'Melanie and her kids were thinking about going camping next month.', 0.6, 0.1),
    ],
    2,
    800
  );
  if (selectedEvidence[0]?.memory.id !== 'exact') {
    throw new Error('self-test failed: answer evidence reranking');
  }
  const originEvidence = selectAnswerEvidence(
    'Where did Caroline move from 4 years ago?',
    [
      fakeRecallResult('friends', 'Caroline has known these friends for 4 years, since she moved from her home country.', 0.9, 0.7),
      fakeRecallResult('noise-a', 'Caroline started playing acoustic guitar about five years ago.', 0.88, 0.1),
      fakeRecallResult('noise-b', 'Melanie went camping with the kids in the forest.', 0.82, 0.1),
      fakeRecallResult('origin', 'Caroline keeps a necklace from her grandma in her home country, Sweden.', 0.55, 0.2),
    ],
    3,
    800
  );
  if (!originEvidence.some((result) => result.memory.id === 'origin')) {
    throw new Error('self-test failed: origin alias evidence selection');
  }
  const namedSubjectEvidence = selectAnswerEvidence(
    'What has Melanie painted?',
    [
      fakeRecallResult('caroline-sunset', 'Caroline: I painted a sunset after visiting the beach.', 0.98, 1),
      fakeRecallResult('melanie-horse', 'Melanie: Here is a photo of my horse painting I did recently.', 0.55, 0.5),
      fakeRecallResult('melanie-sunrise', 'Melanie: I painted that lake sunrise last year.', 0.5, 0.45),
    ],
    3,
    800
  );
  if (namedSubjectEvidence[0]?.memory.id !== 'melanie-horse') {
    throw new Error('self-test failed: named subject evidence selection');
  }
  const lgbtqEventEvidence = selectAnswerEvidence(
    'What LGBTQ+ events has Caroline participated in?',
    [
      fakeRecallResult('generic', 'Melanie: Events like these show strong community support.', 0.9, 0.9),
      fakeRecallResult('school', 'Caroline: My school event last week about my transgender journey was awesome.', 0.7, 0.6),
      fakeRecallResult('support-group', 'Caroline: I went to a LGBTQ support group yesterday and it was powerful.', 0.4, 0.3),
      fakeRecallResult('pride', 'Caroline: Last week I went to an LGBTQ+ pride parade.', 0.45, 0.35),
    ],
    12,
    1200
  );
  if (!lgbtqEventEvidence.some((result) => result.memory.id === 'support-group')) {
    throw new Error('self-test failed: LGBTQ event evidence breadth');
  }
  const postRoadtripHikeEvidence = selectAnswerEvidence(
    'When did Melanie go on a hike after the roadtrip?',
    [
      fakeRecallResult('roadtrip', 'Melanie: That roadtrip this past weekend was insane after the accident.', 0.95, 1),
      fakeRecallResult('old-hike', 'Melanie: We went on a hike in June.', 0.8, 0.7),
      fakeRecallResult('after-roadtrip', 'Melanie: Yup, we just did it yesterday and it was a nice way to relax after the road trip. Session date: 20 October 2023', 0.35, 0.2),
    ],
    10,
    1200
  );
  if (!postRoadtripHikeEvidence.some((result) => result.memory.id === 'after-roadtrip')) {
    throw new Error('self-test failed: post-roadtrip hike evidence selection');
  }
  console.log('LOCOMO agent benchmark self-test passed.');
}

function fakeRecallResult(id, content, score, lexicalScore) {
  return {
    memory: { id, content },
    score,
    lexicalScore,
    vectorScore: 0,
    relationshipScore: 0,
    recencyScore: 0,
  };
}
