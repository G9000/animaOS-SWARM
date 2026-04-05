/**
 * SDK Basic Agent Example
 * 
 * This example shows how to use the animaOS SDK to create and run a simple agent.
 * 
 * Prerequisites:
 * 1. Build the SDK: bun run build:cli-sdk
 * 2. Start the daemon: bun run daemon
 * 3. Set your API key: export OPENAI_API_KEY="sk-..."
 */

import { createDaemonClient, agent } from '../packages/sdk/dist/index.js';

const client = createDaemonClient();

async function main() {
  console.log('🚀 Creating agent...\n');

  // Create a simple research agent
  const researcher = await client.agents.create(agent({
    name: 'researcher',
    provider: 'openai',
    model: 'gpt-4o-mini',
    apiKey: process.env.OPENAI_API_KEY,
    systemPrompt: 'You are a research assistant. Provide concise, factual answers in 2-3 sentences.',
  }));

  console.log(`✅ Agent created: ${researcher.state.name} (${researcher.state.id})\n`);

  // Run a task
  console.log('📝 Running task: "What are the main types of neural networks?"\n');
  
  const result = await client.agents.run(researcher.state.id, {
    text: 'What are the main types of neural networks?'
  });

  console.log('📊 Result:');
  console.log('─'.repeat(50));
  console.log(result.result.content.text);
  console.log('─'.repeat(50));
  
  if (result.result.usage) {
    console.log('\n📈 Token Usage:');
    console.log(`   Prompt: ${result.result.usage.prompt}`);
    console.log(`   Completion: ${result.result.usage.completion}`);
    console.log(`   Total: ${result.result.usage.total}`);
  }

  console.log('\n✨ Done!');
}

main().catch((error) => {
  console.error('❌ Error:', error.message);
  if (error.message.includes('fetch')) {
    console.error('\n💡 Make sure the daemon is running: bun run daemon');
  }
  process.exit(1);
});
