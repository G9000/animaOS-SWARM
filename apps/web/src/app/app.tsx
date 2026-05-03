import { useState } from 'react';
import { agents, type AgentConfig } from '../lib/api';

export function App() {
  const [name, setName] = useState('');
  const [model, setModel] = useState('');
  const [provider, setProvider] = useState('');
  const [creating, setCreating] = useState(false);
  const [created, setCreated] = useState<{ name: string; id: string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function handleCreate() {
    if (!name.trim()) return;
    setCreating(true);
    setError(null);
    setCreated(null);
    try {
      const config: AgentConfig = {
        name: name.trim(),
        model: model.trim() || 'gpt-4o-mini',
        provider: provider.trim() || 'openai',
      };
      const agent = await agents.create(config);
      setCreated({ name: agent.state.name, id: agent.state.id });
      setName('');
      setModel('');
      setProvider('');
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setCreating(false);
    }
  }

  return (
    <div className="flex min-h-screen flex-col items-center justify-center bg-gray-50 px-4">
      <div className="w-full max-w-sm rounded-2xl border border-gray-200 bg-white p-8 shadow-sm">
        <div className="mb-6 text-center">
          <h1 className="text-xl font-semibold tracking-tight text-gray-900">animaOS</h1>
          <p className="mt-1 text-sm text-gray-500">Create an agent</p>
        </div>

        <div className="space-y-3">
          <input
            className="w-full rounded-lg border border-gray-200 bg-gray-50 px-3 py-2.5 text-sm text-gray-900 outline-none ring-gray-200 transition focus:border-gray-400 focus:ring-2"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Agent name"
          />
          <input
            className="w-full rounded-lg border border-gray-200 bg-gray-50 px-3 py-2.5 text-sm text-gray-900 outline-none ring-gray-200 transition focus:border-gray-400 focus:ring-2"
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder="Model (default: gpt-4o-mini)"
          />
          <input
            className="w-full rounded-lg border border-gray-200 bg-gray-50 px-3 py-2.5 text-sm text-gray-900 outline-none ring-gray-200 transition focus:border-gray-400 focus:ring-2"
            value={provider}
            onChange={(e) => setProvider(e.target.value)}
            placeholder="Provider (default: openai)"
          />
          <button
            onClick={handleCreate}
            disabled={creating || !name.trim()}
            className="w-full rounded-lg bg-gray-900 px-4 py-2.5 text-sm font-medium text-white hover:bg-gray-800 disabled:opacity-40"
          >
            {creating ? 'Creating…' : 'Create agent'}
          </button>
        </div>

        {error && (
          <p className="mt-4 text-center text-sm text-red-600">{error}</p>
        )}

        {created && (
          <p className="mt-4 text-center text-sm text-green-700">
            Created <strong>{created.name}</strong> ({created.id.slice(0, 8)})
          </p>
        )}
      </div>
    </div>
  );
}

export default App;
