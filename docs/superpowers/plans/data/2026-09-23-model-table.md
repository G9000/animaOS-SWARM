# Model metadata for cost estimation (verified 2026-09-23)

Every value was read on **2026-09-23** from the official page linked in its row. For Anthropic, the `claude-api` skill's model/pricing reference (cached 2026-06-24) was checked against the live pages that skill links to. Where they differed, the live page is used.

- **Prices:** USD per 1M tokens, standard synchronous tier, global endpoint, base (short-context) rate. Tier rules are in [Notes](#notes).
- **Legend:** `none` means the provider states there is no separate charge (uncached tokens bill at the input rate). `n/a` means the price is not offered for that model. `unknown` means it is not published on the pages read.
- **Matching:** use **longest-prefix match**. Several short prefixes collide with differently priced models (see Notes).

## anthropic

Specs come from each model page and prices from the pricing page. The cache-write column shows the 5-minute / 1-hour TTL prices. Dated IDs are covered by their prefix: `claude-haiku-4-5-20251001`, `claude-opus-4-5-20251101`, `claude-sonnet-4-5-20250929`. The 4.6-and-later IDs are dateless pinned snapshots.

| model prefix | context | max output | vision | input $/M | output $/M | cached input $/M | cache write $/M | source |
|---|---|---|---|---|---|---|---|---|
| `claude-fable-5-1` | 1M | 128K | true | 10 | 50 | 0.25 | 12.50 / 20 | [pricing][a-p] · [model][a-fable-5-1] |
| `claude-opus-5-5` | 1M | 128K | true | 4 | 20 | 0.20 | 5 / 8 | [pricing][a-p] · [model][a-opus-5-5] |
| `claude-sonnet-5` | 1M | 128K | true | 2 | 10 | 0.20 | 2.50 / 4 | [pricing][a-p] · [model][a-sonnet-5] |
| `claude-haiku-4-5` | 200K | 64K | true | 1 | 5 | 0.10 | 1.25 / 2 | [pricing][a-p] · [model][a-haiku-4-5] |
| `claude-fable-5` | 1M | 128K | true | 10 | 50 | 1 | 12.50 / 20 | [pricing][a-p] · [model][a-fable-5] |
| `claude-opus-5` | 1M | 128K | true | 5 | 25 | 0.50 | 6.25 / 10 | [pricing][a-p] · [model][a-opus-5] |
| `claude-opus-4-8` | 1M | 128K | true | 5 | 25 | 0.50 | 6.25 / 10 | [pricing][a-p] · [model][a-opus-4-8] |
| `claude-opus-4-7` | 1M | 128K | true | 5 | 25 | 0.50 | 6.25 / 10 | [pricing][a-p] · [model][a-opus-4-7] |
| `claude-opus-4-6` | 1M | 128K | true | 5 | 25 | 0.50 | 6.25 / 10 | [pricing][a-p] · [model][a-opus-4-6] |
| `claude-opus-4-5` | 200K | 64K | true | 5 | 25 | 0.50 | 6.25 / 10 | [pricing][a-p] · [model][a-opus-4-5] |
| `claude-sonnet-4-6` | 1M | 128K | true | 3 | 15 | 0.30 | 3.75 / 6 | [pricing][a-p] · [model][a-sonnet-4-6] |
| `claude-sonnet-4-5` | 200K | 64K | true | 3 | 15 | 0.30 | 3.75 / 6 | [pricing][a-p] · [model][a-sonnet-4-5] |

[a-p]: https://platform.claude.com/docs/en/about-claude/pricing
[a-fable-5-1]: https://platform.claude.com/docs/en/models/fable-5-1/overview
[a-opus-5-5]: https://platform.claude.com/docs/en/models/opus-5-5/overview
[a-sonnet-5]: https://platform.claude.com/docs/en/models/sonnet-5/overview
[a-haiku-4-5]: https://platform.claude.com/docs/en/models/haiku-4-5/overview
[a-fable-5]: https://platform.claude.com/docs/en/models/fable-5/overview
[a-opus-5]: https://platform.claude.com/docs/en/models/opus-5/overview
[a-opus-4-8]: https://platform.claude.com/docs/en/models/opus-4-8/overview
[a-opus-4-7]: https://platform.claude.com/docs/en/models/opus-4-7/overview
[a-opus-4-6]: https://platform.claude.com/docs/en/models/opus-4-6/overview
[a-opus-4-5]: https://platform.claude.com/docs/en/models/opus-4-5/overview
[a-sonnet-4-6]: https://platform.claude.com/docs/en/models/sonnet-4-6/overview
[a-sonnet-4-5]: https://platform.claude.com/docs/en/models/sonnet-4-5/overview

## openai

Prices come from the pricing page's Standard table and specs from each model page. Each prefix also covers its dated snapshots (for example `gpt-5.5-2026-04-23` and `o3-2025-04-16`). The one exception is `gpt-4o-2024-05-13`, which is priced separately and has its own row.

| model prefix | context | max output | vision | input $/M | output $/M | cached input $/M | cache write $/M | source |
|---|---|---|---|---|---|---|---|---|
| `gpt-6-astra` | 1,050,000 (max in 922,000) | 128,000 | true | 10 | 50 | 1 | 12.50 | [pricing][o-p] · [model][o-gpt-6-astra] |
| `gpt-6-sol` | 1,050,000 (max in 922,000) | 128,000 | true | 2 | 10 | 0.20 | 2.50 | [pricing][o-p] · [model][o-gpt-6-sol] |
| `gpt-6-luna` | 1,050,000 (max in 922,000) | 128,000 | true | 0.10 | 0.50 | 0.01 | 0.125 | [pricing][o-p] · [model][o-gpt-6-luna] |
| `gpt-5.6-sol` | 1,050,000 (max in 922,000) | 128,000 | true | 4 | 20 | 0.40 | 5 | [pricing][o-p] · [model][o-gpt-5.6-sol] |
| `gpt-5.6-terra` | 1,050,000 (max in 922,000) | 128,000 | true | 2 | 12 | 0.20 | 2.50 | [pricing][o-p] · [model][o-gpt-5.6-terra] |
| `gpt-5.6-luna` | 1,050,000 (max in 922,000) | 128,000 | true | 0.20 | 1.20 | 0.02 | 0.25 | [pricing][o-p] · [model][o-gpt-5.6-luna] |
| `gpt-5.5` | 1,050,000 | 128,000 | true | 5 | 30 | 0.50 | none | [pricing][o-p] · [model][o-gpt-5.5] |
| `gpt-5.5-pro` | 1,050,000 | 128,000 | true | 30 | 180 | n/a | none | [pricing][o-p] · [model][o-gpt-5.5-pro] |
| `gpt-5.4` | 1,050,000 | 128,000 | true | 2.50 | 15 | 0.25 | none | [pricing][o-p] · [model][o-gpt-5.4] |
| `gpt-5.4-mini` | 400,000 (max in 272,000) | 128,000 | true | 0.75 | 4.50 | 0.075 | none | [pricing][o-p] · [model][o-gpt-5.4-mini] |
| `gpt-5.4-nano` | 400,000 (max in 272,000) | 128,000 | true | 0.20 | 1.25 | 0.02 | none | [pricing][o-p] · [model][o-gpt-5.4-nano] |
| `gpt-5.4-pro` | 1,050,000 | 128,000 | true | 30 | 180 | n/a | none | [pricing][o-p] · [model][o-gpt-5.4-pro] |
| `gpt-5.2` | 400,000 | 128,000 | true | 1.75 | 14 | 0.175 | none | [pricing][o-p] · [model][o-gpt-5.2] |
| `gpt-5.2-pro` | 400,000 | 128,000 | true | 21 | 168 | n/a | none | [pricing][o-p] · [model][o-gpt-5.2-pro] |
| `gpt-5.1` | 400,000 | 128,000 | true | 1.25 | 10 | 0.125 | none | [pricing][o-p] · [model][o-gpt-5.1] |
| `gpt-5` | 400,000 (max in 272,000) | 128,000 | true | 1.25 | 10 | 0.125 | none | [pricing][o-p] · [model][o-gpt-5] |
| `gpt-5-mini` | 400,000 (max in 272,000) | 128,000 | true | 0.25 | 2 | 0.025 | none | [pricing][o-p] · [model][o-gpt-5-mini] |
| `gpt-5-nano` | 400,000 (max in 272,000) | 128,000 | true | 0.05 | 0.40 | 0.005 | none | [pricing][o-p] · [model][o-gpt-5-nano] |
| `gpt-5-pro` | 400,000 | 272,000 | true | 15 | 120 | n/a | none | [pricing][o-p] · [model][o-gpt-5-pro] |
| `gpt-4.1` | 1,047,576 | 32,768 | true | 2 | 8 | 0.50 | none | [pricing][o-p] · [model][o-gpt-4.1] |
| `gpt-4.1-mini` | 1,047,576 | 32,768 | true | 0.40 | 1.60 | 0.10 | none | [pricing][o-p] · [model][o-gpt-4.1-mini] |
| `gpt-4.1-nano` | 1,047,576 | 32,768 | true | 0.10 | 0.40 | 0.025 | none | [pricing][o-p] · [model][o-gpt-4.1-nano] |
| `gpt-4o` | 128,000 | 16,384 | true | 2.50 | 10 | 1.25 | none | [pricing][o-p] · [model][o-gpt-4o] |
| `gpt-4o-2024-05-13` | 128,000 † | unknown | true † | 5 | 15 | n/a | none | [pricing][o-p] · [model][o-gpt-4o] |
| `gpt-4o-mini` | 128,000 | 16,384 | true | 0.15 | 0.60 | 0.075 | none | [pricing][o-p] · [model][o-gpt-4o-mini] |
| `o3` | 200,000 | 100,000 | true | 2 | 8 | 0.50 | none | [pricing][o-p] · [model][o-o3] |
| `o3-pro` | 200,000 | 100,000 | true | 20 | 80 | n/a | none | [pricing][o-p] · [model][o-o3-pro] |
| `o4-mini` | 200,000 | 100,000 | true | 1.10 | 4.40 | 0.275 | none | [pricing][o-p] · [model][o-o4-mini] |
| `o3-mini` | 200,000 | 100,000 | false | 1.10 | 4.40 | 0.55 | none | [pricing][o-p] · [model][o-o3-mini] |
| `o1` | 200,000 | 100,000 | true | 15 | 60 | 7.50 | none | [pricing][o-p] · [model][o-o1] |
| `o1-pro` | 200,000 | 100,000 | true | 150 | 600 | n/a | none | [pricing][o-p] · [model][o-o1-pro] |

† These are family-level values from the `gpt-4o` page, which lists this snapshot. OpenAI does not publish snapshot-specific limits.

[o-p]: https://developers.openai.com/api/docs/pricing
[o-gpt-6-astra]: https://developers.openai.com/api/docs/models/gpt-6-astra
[o-gpt-6-sol]: https://developers.openai.com/api/docs/models/gpt-6-sol
[o-gpt-6-luna]: https://developers.openai.com/api/docs/models/gpt-6-luna
[o-gpt-5.6-sol]: https://developers.openai.com/api/docs/models/gpt-5.6-sol
[o-gpt-5.6-terra]: https://developers.openai.com/api/docs/models/gpt-5.6-terra
[o-gpt-5.6-luna]: https://developers.openai.com/api/docs/models/gpt-5.6-luna
[o-gpt-5.5]: https://developers.openai.com/api/docs/models/gpt-5.5
[o-gpt-5.5-pro]: https://developers.openai.com/api/docs/models/gpt-5.5-pro
[o-gpt-5.4]: https://developers.openai.com/api/docs/models/gpt-5.4
[o-gpt-5.4-mini]: https://developers.openai.com/api/docs/models/gpt-5.4-mini
[o-gpt-5.4-nano]: https://developers.openai.com/api/docs/models/gpt-5.4-nano
[o-gpt-5.4-pro]: https://developers.openai.com/api/docs/models/gpt-5.4-pro
[o-gpt-5.2]: https://developers.openai.com/api/docs/models/gpt-5.2
[o-gpt-5.2-pro]: https://developers.openai.com/api/docs/models/gpt-5.2-pro
[o-gpt-5.1]: https://developers.openai.com/api/docs/models/gpt-5.1
[o-gpt-5]: https://developers.openai.com/api/docs/models/gpt-5
[o-gpt-5-mini]: https://developers.openai.com/api/docs/models/gpt-5-mini
[o-gpt-5-nano]: https://developers.openai.com/api/docs/models/gpt-5-nano
[o-gpt-5-pro]: https://developers.openai.com/api/docs/models/gpt-5-pro
[o-gpt-4.1]: https://developers.openai.com/api/docs/models/gpt-4.1
[o-gpt-4.1-mini]: https://developers.openai.com/api/docs/models/gpt-4.1-mini
[o-gpt-4.1-nano]: https://developers.openai.com/api/docs/models/gpt-4.1-nano
[o-gpt-4o]: https://developers.openai.com/api/docs/models/gpt-4o
[o-gpt-4o-mini]: https://developers.openai.com/api/docs/models/gpt-4o-mini
[o-o3]: https://developers.openai.com/api/docs/models/o3
[o-o3-pro]: https://developers.openai.com/api/docs/models/o3-pro
[o-o4-mini]: https://developers.openai.com/api/docs/models/o4-mini
[o-o3-mini]: https://developers.openai.com/api/docs/models/o3-mini
[o-o1]: https://developers.openai.com/api/docs/models/o1
[o-o1-pro]: https://developers.openai.com/api/docs/models/o1-pro

## google

Paid-tier Standard prices come from the pricing page and specs from each model page. The input price shown is for text, image and video; audio input costs more on some models (see Notes). The output price includes thinking tokens. Google publishes no per-token cache-write price, only an hourly storage fee for explicit caches, which is shown in the cache-write column.

| model prefix | context | max output | vision | input $/M | output $/M | cached input $/M | cache write $/M | source |
|---|---|---|---|---|---|---|---|---|
| `gemini-3.8-flash` | 1,048,576 | 65,536 | true | 0.75 | 3.75 | 0.075 | unknown (storage 0.50/M/hr) | [pricing][g-p] · [model][g-3.8-flash] |
| `gemini-3.7-flash` | 1,048,576 | 65,536 | true | 0.75 | 3.75 | 0.075 | unknown (storage 0.50/M/hr) | [pricing][g-p] · [model][g-3.7-flash] |
| `gemini-3.6-flash` | 1,048,576 | 65,536 | true | 0.75 | 3.75 | 0.075 | unknown (storage 0.50/M/hr) | [pricing][g-p] · [model][g-3.6-flash] |
| `gemini-3.5-flash` | 1,048,576 | 65,536 | true | 1.50 | 9 | 0.15 | unknown (storage 1.00/M/hr) | [pricing][g-p] · [model][g-3.5-flash] |
| `gemini-3.5-flash-lite` | 1,048,576 | 65,536 | true | 0.30 | 2.50 | 0.03 | unknown (storage 1.00/M/hr) | [pricing][g-p] · [model][g-3.5-flash-lite] |
| `gemini-3.1-flash-lite` | 1,048,576 | 65,536 | true | 0.25 | 1.50 | 0.025 | unknown (storage 1.00/M/hr) | [pricing][g-p] · [model][g-3.1-flash-lite] |
| `gemini-3.1-pro-preview` | 1,048,576 | 65,536 | true | 2 | 12 | 0.20 | unknown (storage 4.50/M/hr) | [pricing][g-p] · [model][g-3.1-pro-preview] |
| `gemini-3-flash-preview` | 1,048,576 | 65,536 | true | 0.50 | 3 | 0.05 | unknown (storage 1.00/M/hr) | [pricing][g-p] · [model][g-3-flash-preview] |
| `gemini-2.5-pro` | 1,048,576 | 65,536 | true | 1.25 | 10 | 0.125 | unknown (storage 4.50/M/hr) | [pricing][g-p] · [model][g-2.5-pro] |
| `gemini-2.5-flash` | 1,048,576 | 65,536 | true | 0.30 | 2.50 | 0.03 | unknown (storage 1.00/M/hr) | [pricing][g-p] · [model][g-2.5-flash] |
| `gemini-2.5-flash-lite` | 1,048,576 | 65,536 | true | 0.10 | 0.40 | 0.01 | unknown (storage 1.00/M/hr) | [pricing][g-p] · [model][g-2.5-flash-lite] |

[g-p]: https://ai.google.dev/gemini-api/docs/pricing
[g-3.8-flash]: https://ai.google.dev/gemini-api/docs/models/gemini-3.8-flash
[g-3.7-flash]: https://ai.google.dev/gemini-api/docs/models/gemini-3.7-flash
[g-3.6-flash]: https://ai.google.dev/gemini-api/docs/models/gemini-3.6-flash
[g-3.5-flash]: https://ai.google.dev/gemini-api/docs/models/gemini-3.5-flash
[g-3.5-flash-lite]: https://ai.google.dev/gemini-api/docs/models/gemini-3.5-flash-lite
[g-3.1-flash-lite]: https://ai.google.dev/gemini-api/docs/models/gemini-3.1-flash-lite
[g-3.1-pro-preview]: https://ai.google.dev/gemini-api/docs/models/gemini-3.1-pro-preview
[g-3-flash-preview]: https://ai.google.dev/gemini-api/docs/models/gemini-3-flash-preview
[g-2.5-pro]: https://ai.google.dev/gemini-api/docs/models/gemini-2.5-pro
[g-2.5-flash]: https://ai.google.dev/gemini-api/docs/models/gemini-2.5-flash
[g-2.5-flash-lite]: https://ai.google.dev/gemini-api/docs/models/gemini-2.5-flash-lite

## deepseek

Prices are shown as **peak / off-peak**. Off-peak is half of peak (see Notes for the hours). DeepSeek bills "number of tokens × price" using only its cache-hit, cache-miss (input) and output prices. No cache-write price exists: uncached tokens bill at the cache-miss rate.

| model prefix | context | max output | vision | input $/M | output $/M | cached input $/M | cache write $/M | source |
|---|---|---|---|---|---|---|---|---|
| `deepseek-flash` (legacy names `deepseek-v4-flash`, `deepseek-v4-flash-vision-exp` bill as this) | 1M | 384K | true | 0.30 / 0.15 | 1.20 / 0.60 | 0.006 / 0.003 | none | [pricing][d-p] |
| `deepseek-v4-pro` | 1M | 384K | false | 1.32 / 0.66 | 3.96 / 1.98 | 0.044 / 0.022 | none | [pricing][d-p] |

[d-p]: https://api-docs.deepseek.com/quick_start/pricing
[d-news]: https://api-docs.deepseek.com/updates

## xai

Prices come from the pricing page (below the 200k-prompt threshold) and specs from each model page. The billing table on xAI's caching page lists no cache-write charge: uncached prompt tokens bill at the full input price.

| model prefix | context | max output | vision | input $/M | output $/M | cached input $/M | cache write $/M | source |
|---|---|---|---|---|---|---|---|---|
| `grok-4.7` | 500,000 | none ("no text output limit") | true | 2 | 6 | 0.50 | none | [pricing][x-p] · [model][x-4.7] · [guide][x-4.7g] |
| `grok-4.6` | 500,000 | none ("no text output limit") | true | 2 | 6 | 0.50 | none | [pricing][x-p] · [model][x-4.6] · [notes][x-rn] |
| `grok-4.5` (aliases `grok-4.5-latest`, `grok-build-latest`) | 500,000 | unknown | true | 2 | 6 | 0.30 | none | [pricing][x-p] · [model][x-4.5] |
| `grok-4.3` (alias `grok-4.3-latest`) | 1,000,000 | unknown | true | 1.25 | 2.50 | 0.20 | none | [pricing][x-p] · [model][x-4.3] |
| `grok-4.20` (covers `grok-4.20-0309-reasoning`, `-0309-non-reasoning`, `-multi-agent-0309` and their aliases) | 1,000,000 | unknown | true | 1.25 | 2.50 | 0.20 | none | [pricing][x-p] · [r][x-4.20r] · [nr][x-4.20nr] · [ma][x-4.20ma] |
| `grok-build-0.1` (aliases `grok-code-fast-1`, `grok-code-fast`, `grok-code-fast-1-0825`) | 256,000 | unknown | true | 1 | 2 | 0.20 | none | [pricing][x-p] · [model][x-build] |

[x-p]: https://docs.x.ai/developers/pricing
[x-4.7]: https://docs.x.ai/developers/models/grok-4.7
[x-4.7g]: https://docs.x.ai/developers/grok-4-7
[x-4.6]: https://docs.x.ai/developers/models/grok-4.6
[x-rn]: https://docs.x.ai/developers/release-notes
[x-4.5]: https://docs.x.ai/developers/models/grok-4.5
[x-4.3]: https://docs.x.ai/developers/models/grok-4.3
[x-4.20r]: https://docs.x.ai/developers/models/grok-4.20-0309-reasoning
[x-4.20nr]: https://docs.x.ai/developers/models/grok-4.20-0309-non-reasoning
[x-4.20ma]: https://docs.x.ai/developers/models/grok-4.20-multi-agent-0309
[x-build]: https://docs.x.ai/developers/models/grok-build-0.1
[x-cache]: https://docs.x.ai/developers/advanced-api-usage/prompt-caching/usage-and-pricing
[x-ret]: https://docs.x.ai/developers/migration/may-15-retirement

## mistral

Standard prices come from the docs pricing page and specs (API names, context, modalities) from each model page. Cached input is billed at 10% of the input price.

| model prefix | context | max output | vision | input $/M | output $/M | cached input $/M | cache write $/M | source |
|---|---|---|---|---|---|---|---|---|
| `mistral-medium-3-5` (aliases `mistral-medium-3`, `mistral-medium-latest`) | 256k | unknown | true | 1.50 | 7.50 | 0.15 | unknown | [pricing][m-p] · [model][m-medium-3.5] |
| `mistral-large-2512` (alias `mistral-large-latest`) | 256k | unknown | true | 0.50 | 1.50 | 0.05 | unknown | [pricing][m-p] · [model][m-large-3] |
| `mistral-small-2603` (alias `mistral-small-latest`) | 256k | unknown | true | 0.15 | 0.60 | 0.015 | unknown | [pricing][m-p] · [model][m-small-4] |
| `ministral-14b-2512` (alias `ministral-14b-latest`) | 256k | unknown | true | 0.20 | 0.20 | 0.02 | unknown | [pricing][m-p] · [model][m-14b] |
| `ministral-8b-2512` (alias `ministral-8b-latest`) | 256k | unknown | true | 0.15 | 0.15 | 0.015 | unknown | [pricing][m-p] · [model][m-8b] |
| `ministral-3b-2512` (alias `ministral-3b-latest`) | 256k | unknown | true | 0.10 | 0.10 | 0.01 | unknown | [pricing][m-p] · [model][m-3b] |
| `codestral-2508` (alias `codestral-latest`) | 128k | unknown | false | 0.30 | 0.90 | 0.03 | unknown | [pricing][m-p] · [model][m-codestral] |
| `zai-glm-5-3` (aliases `zai-glm-5`, `zai-glm-latest`; third-party) | 1M | unknown | false | 1.40 | 4.40 | 0.14 | unknown | [pricing][m-p] · [model][m-glm-5.3] |
| `zai-glm-5-2` (third-party) | 1M | unknown | false | 1.40 | 4.40 | 0.14 | unknown | [pricing][m-p] · [model][m-glm-5.2] |

[m-p]: https://docs.mistral.ai/inference/pricing
[m-medium-3.5]: https://docs.mistral.ai/models/mistral-medium-3-5-26-04
[m-large-3]: https://docs.mistral.ai/models/mistral-large-3-25-12
[m-small-4]: https://docs.mistral.ai/models/mistral-small-4-0-26-03
[m-14b]: https://docs.mistral.ai/models/ministral-3-14b-25-12
[m-8b]: https://docs.mistral.ai/models/ministral-3-8b-25-12
[m-3b]: https://docs.mistral.ai/models/ministral-3-3b-25-12
[m-codestral]: https://docs.mistral.ai/models/codestral-25-08
[m-glm-5.3]: https://docs.mistral.ai/models/zai-glm-5-3
[m-glm-5.2]: https://docs.mistral.ai/models/zai-glm-5-2
[m-models]: https://docs.mistral.ai/models
[m-cache]: https://docs.mistral.ai/studio/conversations/advanced/prompt-caching
[m-reg]: https://docs.mistral.ai/inference/regional-inference

## Notes

### Prefix collisions (use longest match)

- **Collisions where the shorter prefix has a different price:**
  - `claude-opus-5` also matches `claude-opus-5-5`.
  - `claude-fable-5` also matches `claude-fable-5-1` (the cache-read price differs).
  - `gpt-5` matches every `gpt-5-*` and `gpt-5.x*` id.
  - `gpt-5.5` matches `gpt-5.5-pro`; `gpt-5.4` matches `-mini`, `-nano` and `-pro`.
  - `gpt-4o` matches `gpt-4o-mini` and `gpt-4o-2024-05-13`.
  - `o3` matches `o3-pro` and `o3-mini`; `o1` matches `o1-pro`.
  - `gemini-2.5-flash` matches `-lite`; `gemini-3.5-flash` matches `-lite`.
  - `grok-build-latest` is an alias of **grok-4.5**, not `grok-build-0.1`, so do not match on a bare `grok-build`.
- **Priced but not tabulated.** These fall through to a shorter prefix at the wrong price unless you add rows for them:
  - OpenAI `gpt-5.6-cyber`: $12.50 input, $1.25 cached, $15.625 cache write, $75 output.
  - OpenAI `gpt-5.5-cyber`: $12.50 input, $1.25 cached, $75 output.
  - OpenAI `gpt-5.3-codex`: $1.75 input, $0.175 cached, $14 output.
  - OpenAI `chat-latest`: $5 input, $0.50 cached, $30 output.
  - OpenAI `gpt-5-search-api`: $1.25 input, $0.125 cached, $10 output.
  - The Gemini `-image`, `-tts`, `-live` and `-native-audio` models are not text models and are not priced here.

### Tier and modifier rules

- **anthropic**
  - No long-context tier: 4.6-and-later models include the full 1M context at standard price.
  - Cache writes cost 1.25x input for the 5-minute TTL and 2x for the 1-hour TTL.
  - Cache reads cost 0.1x input, except 0.025x on Fable 5.1 and 0.05x on Opus 5.5.
  - Batch is 50% off.
  - `inference_geo: "us"` multiplies every category by 1.1x (4.6 and later).
  - Fast mode (research preview, Claude API only) costs $8 input / $40 output on Opus 5.5 and $10 / $50 on Opus 5 and 4.8.
- **openai**
  - For `gpt-6-*` and `gpt-5.6-*`, a prompt over **272K input tokens** bills the whole request at 2x input, cached-input and cache-write rates and 1.5x output. Long-context rates (input / cached / write / output):

    | model | input | cached | write | output |
    |---|---|---|---|---|
    | gpt-6-astra | 20 | 2 | 25 | 75 |
    | gpt-6-sol | 4 | 0.40 | 5 | 15 |
    | gpt-6-luna | 0.20 | 0.02 | 0.25 | 0.75 |
    | gpt-5.6-sol | 8 | 0.80 | 10 | 30 |
    | gpt-5.6-terra | 4 | 0.40 | 5 | 18 |
    | gpt-5.6-luna | 0.40 | 0.04 | 0.50 | 1.80 |

  - For `gpt-5.5`, `gpt-5.5-pro`, `gpt-5.4` and `gpt-5.4-pro`, a prompt over 272K bills at 2x input and 1.5x output. Long-context rates (input / cached / output): gpt-5.5 10 / 1 / 45; gpt-5.4 5 / 0.50 / 22.50; both pro models 60 / – / 270.
  - No other row has a long-context tier.
  - Cache writes cost 1.25x input on GPT-5.6 and later only. For GPT-5.5 and earlier the caching guide says "No additional cache-write charge".
  - GPT-5.6 Sol's $4 / $20 price is promotional "at least through November 21, 2026".
  - Batch and Flex are 50% of Standard.
  - Fast mode (renamed from Priority on 2026-07-30) is 2x on GPT-6 and GPT-5.6; other models vary.
  - Data-residency endpoints add 10% for models released on or after 2026-03-05.
- **google**
  - Prompts over 200k: `gemini-3.1-pro-preview` bills 4 / 18 / cached 0.40; `gemini-2.5-pro` bills 2.50 / 15 / cached 0.25.
  - The prices shown for 3.8, 3.7 and 3.6 Flash apply through 2026-12-31. From 2027-01-01 they become 1.50 / 7.50 / cached 0.15, with storage at 1.00/M/hr.
  - Audio input costs more on some models: 3.1-flash-lite 0.50 (cached 0.05), 3-flash-preview 1.00 (cached 0.10), 2.5-flash 1.00 (cached 0.10), 2.5-flash-lite 0.30 (cached 0.03).
  - Batch and Flex are 50% of Standard for 3.8 Flash.
- **deepseek**: peak hours are 01:00–04:00 and 06:00–10:00 UTC, Monday to Friday, excluding Chinese public holidays. All other hours are off-peak at 50% of peak.
- **xai**
  - When the prompt reaches **200k tokens or more**, every token in the request bills at the long-context rate (input / cached / output): 4.7 and 4.6 at 4 / 1 / 12; 4.5 at 4 / 0.60 / 12; 4.3 and 4.20 at 2.50 / 0.40 / 5; build-0.1 at 2 / 0.40 / 4.
  - Priority is 2x.
  - The US regional endpoint is 1.1x and serves 4.7 and 4.6 only.
  - Batch is 20% off, for 4.3 and 4.20 only.
- **mistral**: regional inference is 1.1x on all token categories. The Batch and Priority tabs exist on the pricing page but were not recorded.

### Not verifiable (marked `unknown`)

- **Max output:** Mistral publishes none. xAI publishes none except for 4.7 and 4.6, which are documented as having "no text output limit".
- **Cache write:**
  - Google has no per-token write price; the caching page covers implicit caching only.
  - Mistral's caching page bills uncached tokens at the standard input rate and lists no write fee, but its regional-inference page names "cache writes" as a billed category, and no write price is published.
- **Exact integers:** DeepSeek (1M, 384K), Mistral (256k, 128k, 1M) and Anthropic (1M, 200K, 128K, 64K) state limits only as abbreviations.

### Lifecycle, exclusions and ambiguities

- **anthropic**
  - Excluded `claude-mythos-5-1` and `claude-mythos-5`. Both are limited availability (Project Glasswing) and are priced the same as Fable 5.1 and Fable 5.
  - Retired on the Claude API: Opus 4.1 (2026-08-05), Opus 4 and Sonnet 4 (2026-06-15), Haiku 3 (2026-04-20), Haiku 3.5 and Sonnet 3.7 (2026-02-19). The pricing page says some of these remain on Bedrock or Google Cloud.
  - Earliest retirement dates: Sonnet 4.5 not before 2026-09-29, Haiku 4.5 not before 2026-10-15, Opus 4.5 not before 2026-11-24.
  - The skill's cached reference (2026-06-24) still calls Opus 5.5 "launching" and lists Opus 4.1 and Sonnet 4 as merely deprecated. The live pages show Opus 5.5 as GA since 2026-09-22 and those two as retired. All of the skill's prices matched the live pages.
- **openai** ([deprecations](https://developers.openai.com/api/docs/deprecations), [caching guide](https://developers.openai.com/api/docs/guides/prompt-caching))
  - Shutdown **2026-10-23**: `gpt-4.1-nano`, `gpt-4o-2024-05-13`, `o1`, `o1-pro`, `o3-mini` and `o4-mini`. Also `gpt-4`, `gpt-4-turbo` and `gpt-3.5-turbo`, which are not tabulated.
  - Shutdown **2026-12-11**: the `gpt-5-2025-08-07`, `gpt-5-mini`, `gpt-5-nano`, `gpt-5-pro`, `o3` and `o3-pro` snapshots.
- **google** ([models](https://ai.google.dev/gemini-api/docs/models), [caching](https://ai.google.dev/gemini-api/docs/caching))
  - `gemini-3.1-pro-preview` and `gemini-3-flash-preview` are Preview; there is no GA 3.x Pro.
  - Access to the 2.5 models is limited to users who actively used them in the past.
  - `gemini-3.1-pro-preview-customtools` is priced the same as `gemini-3.1-pro-preview`.
  - The `gemini-3.5-flash` page lists `gemini-3-flash-preview` as its preview version, but the pricing page prices the two differently. The table uses the pricing page.
- **deepseek**
  - `deepseek-chat` and `deepseek-reasoner` were announced for discontinuation on 2026-07-24 ([news][d-news], 2026-04-24) and are not on the current pricing page, so they are excluded. Their current status is unverified.
  - V4 Pro service continues past 2026-09-14 per the 2026-09-10 news.
- **xai** ([retirement guide][x-ret], [caching][x-cache])
  - The retired slugs `grok-4-1-fast-*`, `grok-4-fast-*`, `grok-4-0709` and `grok-3` now redirect to `grok-4.3` and bill at grok-4.3 rates.
  - `grok-code-fast-1` routes to `grok-build-0.1`, which lists it as an alias. The guide's generic statement that deprecated slugs bill at grok-4.3 rates conflicts with this, so this row is ambiguous.
  - Grok 4.7 Fast is not on the public API.
- **mistral** ([models][m-models], [caching][m-cache], [regional][m-reg])
  - The GLM rows are third-party models hosted by Mistral.
  - All Magistral, Devstral, Medium 3 and 3.1, Small 3.x, Nemo, Pixtral and Large 2.x models were retired by 2026-08-31 and are excluded.
  - `mistral-medium-3` is now an alias of Medium 3.5.
