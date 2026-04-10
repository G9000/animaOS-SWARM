import { expect, test, type Page } from '@playwright/test';

const serverOrigin = 'http://127.0.0.1:4300';

async function expectLiveDashboard(page: Page) {
  await page.goto('/');

  await expect(
    page.getByRole('heading', { level: 1, name: 'ANIMAOS CONTROL GRID' })
  ).toBeVisible();
  await expect(page.locator('aside')).toContainText('LIVE');
  await expect(page.locator('main')).toContainText('connection live-streaming');
}

function entityCard(page: Page, text: string) {
  return page.locator('button').filter({ hasText: text }).first();
}

function agentOutputPane(page: Page) {
  return page.getByText('last output').locator('xpath=..').locator('pre');
}

function swarmOutputPane(page: Page) {
  return page.getByText('last swarm output').locator('xpath=..').locator('pre');
}

function navButton(page: Page, text: string) {
  return page.locator('button').filter({ hasText: text }).first();
}

function searchResultCard(page: Page, text: string) {
  return page.locator('article').filter({ hasText: text }).first();
}

test('reflects externally created agents over the live event stream', async ({
  browserName,
  page,
  request,
}) => {
  const agentName = `live-agent-${browserName}-${Date.now().toString(36)}`;
  let createdAgentId: string | null = null;

  try {
    await expectLiveDashboard(page);

    const response = await request.post(`${serverOrigin}/api/agents`, {
      data: {
        name: agentName,
        model: 'gpt-5.4',
      },
    });

    expect(response.ok()).toBeTruthy();
    const created = (await response.json()) as {
      id: string;
      name: string;
      status: string;
    };
    createdAgentId = created.id;

    await expect(entityCard(page, agentName)).toBeVisible({ timeout: 5_000 });
    await expect(page.locator('main')).toContainText(
      `Live agent detected: ${agentName}`
    );
  } finally {
    if (createdAgentId) {
      await request.delete(`${serverOrigin}/api/agents/${createdAgentId}`);
    }
  }
});

test('reflects externally created swarms over the live event stream', async ({
  browserName,
  page,
  request,
}) => {
  const runId = `${browserName}-${Date.now().toString(36)}`;

  await expectLiveDashboard(page);
  await page.getByRole('button', { name: /Swarms/i }).click();

  const response = await request.post(`${serverOrigin}/api/swarms`, {
    data: {
      strategy: 'supervisor',
      manager: {
        name: `manager-${runId}`,
        model: 'gpt-5.4',
      },
      workers: [
        {
          name: `worker-${runId}`,
          model: 'gpt-5.4',
        },
      ],
    },
  });

  expect(response.ok()).toBeTruthy();
  const created = (await response.json()) as {
    id: string;
    strategy: string;
  };

  await expect(entityCard(page, created.id)).toBeVisible({ timeout: 5_000 });
  await expect(page.locator('main')).toContainText(created.id);
  await expect(page.locator('main')).toContainText(
    'Live swarm staged: supervisor'
  );
});

test('streams externally started agent task progress into the browser', async ({
  browserName,
  page,
  request,
}) => {
  const agentName = `task-agent-${browserName}-${Date.now().toString(36)}`;
  let createdAgentId: string | null = null;

  try {
    await expectLiveDashboard(page);

    const createResponse = await request.post(`${serverOrigin}/api/agents`, {
      data: {
        name: agentName,
        model: 'gpt-5.4',
      },
    });

    expect(createResponse.ok()).toBeTruthy();
    const created = (await createResponse.json()) as {
      id: string;
      name: string;
      status: string;
    };
    createdAgentId = created.id;

    const agentButton = entityCard(page, agentName);
    await expect(agentButton).toBeVisible({ timeout: 5_000 });
    await agentButton.click();
    await expect(page.locator('main')).toContainText(agentName);

    const runResponse = await request.post(
      `${serverOrigin}/api/agents/${created.id}/run`,
      {
        data: {
          task: 'Summarize live progress for the operator deck.',
        },
      }
    );

    expect(runResponse.ok()).toBeTruthy();

    await expect(page.locator('main')).toContainText(
      'Live agent task started',
      {
        timeout: 5_000,
      }
    );
    await expect(page.locator('main')).toContainText(
      'Live agent task completed',
      {
        timeout: 5_000,
      }
    );

    const outputPane = agentOutputPane(page);
    await expect(outputPane).toContainText(
      '"text": "Mock completion for: Summarize live progress for the operator deck."',
      {
        timeout: 5_000,
      }
    );
  } finally {
    if (createdAgentId) {
      await request.delete(`${serverOrigin}/api/agents/${createdAgentId}`);
    }
  }
});

test('streams externally started swarm task completion into the browser', async ({
  browserName,
  page,
  request,
}) => {
  const runId = `${browserName}-${Date.now().toString(36)}`;

  await expectLiveDashboard(page);
  await page.getByRole('button', { name: /Swarms/i }).click();

  const createResponse = await request.post(`${serverOrigin}/api/swarms`, {
    data: {
      strategy: 'supervisor',
      manager: {
        name: `manager-task-${runId}`,
        model: 'gpt-5.4',
      },
      workers: [
        {
          name: `worker-task-${runId}`,
          model: 'gpt-5.4',
        },
      ],
    },
  });

  expect(createResponse.ok()).toBeTruthy();
  const created = (await createResponse.json()) as {
    id: string;
    strategy: string;
  };

  const swarmButton = entityCard(page, created.id);
  await expect(swarmButton).toBeVisible({ timeout: 5_000 });
  await swarmButton.click();

  const runResponse = await request.post(
    `${serverOrigin}/api/swarms/${created.id}/run`,
    {
      data: {
        task: 'Summarize live swarm progress for the operator deck.',
      },
    }
  );

  expect(runResponse.ok()).toBeTruthy();

  await expect(page.locator('main')).toContainText(
    'Live swarm task completed',
    {
      timeout: 5_000,
    }
  );
  await expect(swarmButton).toContainText('results 1', {
    timeout: 5_000,
  });

  const outputPane = swarmOutputPane(page);
  await expect(outputPane).toContainText('"status": "success"', {
    timeout: 5_000,
  });
  await expect(outputPane).toContainText('"durationMs":', {
    timeout: 5_000,
  });
  await expect(outputPane).toContainText(
    '"text": "Mock completion for: Summarize live swarm progress for the operator deck."',
    {
      timeout: 5_000,
    }
  );
  await expect(page.locator('main')).toContainText(
    'Mock completion for: Summarize live swarm progress for the operator deck.',
    {
      timeout: 5_000,
    }
  );
});

test('finds recorded task history through the browser search workflow', async ({
  browserName,
  page,
  request,
}) => {
  const runId = `${browserName}-${Date.now().toString(36)}`;
  const agentName = `history-agent-${runId}`;
  const taskText = `Trace history probe ${runId} through the operator search workflow.`;
  let createdAgentId: string | null = null;

  try {
    await expectLiveDashboard(page);

    const createResponse = await request.post(`${serverOrigin}/api/agents`, {
      data: {
        name: agentName,
        model: 'gpt-5.4',
      },
    });

    expect(createResponse.ok()).toBeTruthy();
    const created = (await createResponse.json()) as {
      id: string;
      name: string;
      status: string;
    };
    createdAgentId = created.id;

    const runResponse = await request.post(
      `${serverOrigin}/api/agents/${created.id}/run`,
      {
        data: {
          task: taskText,
        },
      }
    );

    expect(runResponse.ok()).toBeTruthy();

    await navButton(page, 'Search').click();
    await page.getByLabel('History query').fill(runId);
    await page.getByRole('button', { name: /search task history/i }).click();

    const resultCard = searchResultCard(page, taskText);
    await expect(resultCard).toBeVisible({ timeout: 5_000 });
    await expect(resultCard).toContainText(taskText);
    await expect(resultCard).toContainText(`Mock completion for: ${taskText}`, {
      timeout: 5_000,
    });
    await expect(resultCard).toContainText('score', {
      timeout: 5_000,
    });
  } finally {
    if (createdAgentId) {
      await request.delete(`${serverOrigin}/api/agents/${createdAgentId}`);
    }
  }
});

test('finds ingested documents through the browser search workflow', async ({
  browserName,
  page,
}) => {
  const runId = `${browserName}-${Date.now().toString(36)}`;
  const documentId = `ops-playbook-${runId}`;
  const documentText = `Escalate incident ${runId} through the live operator deck before paging the overnight reviewer.`;

  await expectLiveDashboard(page);
  await navButton(page, 'Search').click();

  await page.getByLabel('Document id').fill(documentId);
  await page.getByLabel('Document text').fill(documentText);
  await page.getByRole('button', { name: /ingest document/i }).click();

  await expect(page.getByLabel('Document text')).toHaveValue('', {
    timeout: 5_000,
  });

  await page.getByLabel('Document query').fill(runId);
  await page.getByRole('button', { name: /search documents/i }).click();

  const resultCard = searchResultCard(page, documentText);
  await expect(resultCard).toBeVisible({ timeout: 5_000 });
  await expect(resultCard).toContainText(documentId);
  await expect(resultCard).toContainText('chunk 0');
  await expect(resultCard).toContainText(documentText);
  await expect(resultCard).toContainText('score', {
    timeout: 5_000,
  });
  await expect(resultCard).not.toContainText('"docId"');
});

test('finds the matching chunk from a multi-chunk document search workflow', async ({
  browserName,
  page,
}) => {
  const runId = `${browserName}-${Date.now().toString(36)}`;
  const documentId = `ops-handbook-${runId}`;
  const firstChunk = `Baseline handoff policy for ${runId} keeps the warm-start checklist local to the operator deck.`;
  const secondChunk = `Escalation needle ${runId}-needle routes through the secondary review mesh before paging orbital support.`;

  await expectLiveDashboard(page);
  await navButton(page, 'Search').click();

  await page.getByLabel('Document id').fill(documentId);
  await page
    .getByLabel('Document text')
    .fill(`${firstChunk}\n\n${secondChunk}`);
  await page.getByRole('button', { name: /ingest document/i }).click();

  await expect(page.getByLabel('Document text')).toHaveValue('', {
    timeout: 5_000,
  });

  await page.getByLabel('Document query').fill(`${runId}-needle`);
  await page.getByRole('button', { name: /search documents/i }).click();

  const matchingCard = searchResultCard(page, secondChunk);
  await expect(matchingCard).toBeVisible({ timeout: 5_000 });
  await expect(matchingCard).toContainText(documentId);
  await expect(matchingCard).toContainText('chunk 1');
  await expect(matchingCard).toContainText(secondChunk);
  await expect(matchingCard).not.toContainText('"chunkId"');

  const firstChunkCard = searchResultCard(page, firstChunk);
  await expect(firstChunkCard).toBeVisible({ timeout: 5_000 });
  await expect(firstChunkCard).toContainText('chunk 0');
  await expect(
    page.locator('article').filter({ hasText: documentId })
  ).toHaveCount(2);
});
