/**
 * SDK List Agents Example
 * 
 * Shows how to list and inspect all running agents.
 * 
 * Prerequisites:
 * 1. Build the SDK: bun run build:cli-sdk
 * 2. Start the daemon: bun run daemon
 */

import { createDaemonClient, agent } from '../packages/sdk/dist/index.js';

const client = createDaemonClient();

async function main() {
  console.log('📋 Listing all agents...\n');

  // List all agents
  const agents = await client.agents.list();

  if (agents.length === 0) {
    console.log('No agents found. Creating a sample agent...\n');
    
    // Create a sample agent
    const sample = await client.agents.create(agent({
      name: 'sample-agent',
      provider: 'openai',
      model: 'gpt-4o-mini',
      apiKey: process.env.OPENAI_API_KEY,
      systemPrompt: 'You are a sample agent.',
    }));
    
    console.log(`✅ Created sample agent: ${sample.state.id}\n`);
    agents.push(sample);
  }

  console.log(`Found ${agents.length} agent(s):\n`);
  console.log('─'.repeat(80));

  for (const agent of agents) {
    const { state } = agent;
    
    console.log(`👤 ${state.name}`);
    console.log(`   ID:        ${state.id}`);
    console.log(`   Status:    ${state.status}`);
    console.log(`   Provider:  ${state.config.provider}`);
    console.log(`   Model:     ${state.config.model}`);
    console.log(`   Messages:  ${agent.messageCount}`);
    console.log(`   Events:    ${agent.eventCount}`);
    
    if (agent.lastTask) {
      console.log(`   Last Task: ${agent.lastTask.status} (${agent.lastTask.durationMs}ms)`);
    }
    
    console.log('─'.repeat(80));
  }

  // Get detailed info for first agent
  if (agents.length > 0) {
    const firstId = agents[0].state.id;
    console.log(`\n📊 Detailed info for ${agents[0].state.name}:`);
    
    try {
      const detailed = await client.agents.get(firstId);
      console.log('   State:', JSON.stringify(detailed.state, null, 2).slice(0, 500) + '...');
      
      // Get memories
      const memories = await client.agents.recentMemories(firstId, { limit: 3 });
      console.log(`\n   Recent Memories: ${memories.length}`);
      for (const mem of memories) {
        console.log(`   - [${mem.type}] ${mem.content.slice(0, 50)}...`);
      }
    } catch (e) {
      console.log('   (Could not fetch detailed info)');
    }
  }
}

main().catch((error) => {
  console.error('❌ Error:', error.message);
  if (error.message.includes('fetch') || error.message.includes('ECONNREFUSED')) {
    console.error('\n💡 Make sure the daemon is running: bun run daemon');
  }
  process.exit(1);
});
