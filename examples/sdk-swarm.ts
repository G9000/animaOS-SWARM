/**
 * SDK Swarm Example
 * 
 * This example shows how to create a multi-agent swarm that collaborates on a task.
 * 
 * Prerequisites:
 * 1. Build the SDK: bun run build:cli-sdk
 * 2. Start the daemon: bun run daemon
 * 3. Set your API key: export OPENAI_API_KEY="sk-..."
 */

import { createDaemonClient, agent, swarm } from '../packages/sdk/dist/index.js';

const client = createDaemonClient();

async function main() {
  console.log('🚀 Creating content team swarm...\n');

  // Create a swarm with multiple specialized agents
  const contentTeam = await client.swarms.create(swarm({
    name: 'content-team',
    strategy: 'round-robin',
    maxIterations: 6,
    agents: [
      agent({
        name: 'researcher',
        provider: 'openai',
        model: 'gpt-4o-mini',
        apiKey: process.env.OPENAI_API_KEY,
        systemPrompt: 'You are a research specialist. Gather key facts and information about the topic. Be thorough and cite specific details.',
      }),
      agent({
        name: 'writer',
        provider: 'openai',
        model: 'gpt-4o',
        apiKey: process.env.OPENAI_API_KEY,
        systemPrompt: 'You are a content writer. Create engaging, well-structured content based on research. Use markdown formatting.',
      }),
      agent({
        name: 'editor',
        provider: 'openai',
        model: 'gpt-4o-mini',
        apiKey: process.env.OPENAI_API_KEY,
        systemPrompt: 'You are an editor. Review and polish content. Fix grammar, improve clarity, and ensure consistency.',
      }),
    ],
  }));

  console.log(`✅ Swarm created: ${contentTeam.name} (${contentTeam.id})`);
  console.log(`   Agents: ${contentTeam.agents.map(a => a.name).join(', ')}\n`);

  // Run the swarm task with streaming events
  const task = 'Write a short blog post about the benefits of walking for health';
  console.log(`📝 Running task: "${task}"\n`);

  // Subscribe to events for real-time updates
  const eventStream = client.swarms.subscribe(contentTeam.id);
  
  // Start the task
  const runPromise = client.swarms.run(contentTeam.id, { text: task });

  // Process events as they arrive
  let eventCount = 0;
  for await (const event of eventStream) {
    eventCount++;
    const { state, result } = event.data;
    
    if (state.iteration) {
      console.log(`🔄 Iteration ${state.iteration}/${state.maxIterations || '∞'}`);
    }
    
    if (result?.content) {
      const preview = (result.content as any).text?.slice(0, 100) || 'Processing...';
      console.log(`   ${preview}...\n`);
    }
  }

  // Get final result
  const finalResult = await runPromise;
  
  console.log('─'.repeat(60));
  console.log('📄 FINAL CONTENT:');
  console.log('─'.repeat(60));
  console.log(finalResult.result.content.text);
  console.log('─'.repeat(60));

  if (finalResult.result.usage) {
    console.log('\n📈 Token Usage:');
    console.log(`   Total: ${finalResult.result.usage.total}`);
  }

  console.log(`\n✨ Done! Processed ${eventCount} events.`);
}

main().catch((error) => {
  console.error('❌ Error:', error.message);
  if (error.message.includes('fetch') || error.message.includes('ECONNREFUSED')) {
    console.error('\n💡 Make sure the daemon is running: bun run daemon');
  }
  process.exit(1);
});
