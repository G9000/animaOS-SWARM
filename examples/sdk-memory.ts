/**
 * SDK Memory Example
 * 
 * This example demonstrates how agents maintain memory across conversations.
 * 
 * Prerequisites:
 * 1. Build the SDK: bun run build:cli-sdk
 * 2. Start the daemon: bun run daemon
 * 3. Set your API key: export OPENAI_API_KEY="sk-..."
 */

import { createDaemonClient, agent } from '../packages/sdk/dist/index.js';

const client = createDaemonClient();

async function main() {
  console.log('🚀 Creating conversational agent with memory...\n');

  // Create an agent that maintains conversation history
  const assistant = await client.agents.create(agent({
    name: 'memory-assistant',
    provider: 'openai',
    model: 'gpt-4o-mini',
    apiKey: process.env.OPENAI_API_KEY,
    systemPrompt: 'You are a helpful assistant with memory. Remember personal details shared by the user.',
  }));

  console.log(`✅ Agent created: ${assistant.state.name} (${assistant.state.id})\n`);

  // Conversation 1: Share personal information
  console.log('💬 User: "My name is Alice and I love hiking in the mountains."');
  const response1 = await client.agents.run(assistant.state.id, {
    text: 'My name is Alice and I love hiking in the mountains.'
  });
  console.log(`🤖 Agent: ${response1.result.content.text}\n`);

  // Conversation 2: Ask about previous context (memory test)
  console.log('💬 User: "What is my name and what do I enjoy doing?"');
  const response2 = await client.agents.run(assistant.state.id, {
    text: 'What is my name and what do I enjoy doing?'
  });
  console.log(`🤖 Agent: ${response2.result.content.text}\n`);

  // Conversation 3: Add more context
  console.log('💬 User: "I also enjoy reading science fiction books."');
  const response3 = await client.agents.run(assistant.state.id, {
    text: 'I also enjoy reading science fiction books.'
  });
  console.log(`🤖 Agent: ${response3.result.content.text}\n`);

  // Check stored memories
  console.log('🧠 Retrieving agent memories...\n');
  const memories = await client.agents.recentMemories(assistant.state.id, { limit: 10 });
  
  console.log(`📊 Found ${memories.length} memories:\n`);
  
  for (const memory of memories) {
    const importance = '⭐'.repeat(Math.min(memory.importance, 5)) || '⚪';
    console.log(`${importance} [${memory.type}]`);
    console.log(`   Content: ${memory.content.slice(0, 80)}${memory.content.length > 80 ? '...' : ''}`);
    console.log(`   Created: ${new Date(memory.createdAt).toLocaleString()}`);
    if (memory.tags?.length) {
      console.log(`   Tags: ${memory.tags.join(', ')}`);
    }
    console.log();
  }

  // Final memory test
  console.log('💬 User: "Based on everything we discussed, what activities might I enjoy on a weekend?"');
  const response4 = await client.agents.run(assistant.state.id, {
    text: 'Based on everything we discussed, what activities might I enjoy on a weekend?'
  });
  console.log(`🤖 Agent: ${response4.result.content.text}\n`);

  console.log('✨ Memory demonstration complete!');
}

main().catch((error) => {
  console.error('❌ Error:', error.message);
  if (error.message.includes('fetch') || error.message.includes('ECONNREFUSED')) {
    console.error('\n💡 Make sure the daemon is running: bun run daemon');
  }
  process.exit(1);
});
