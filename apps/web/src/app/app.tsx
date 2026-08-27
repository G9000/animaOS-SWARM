import { ViewHarness } from '../ViewHarness';

export function App() {
  return (
    <div className="app-viewport safe-app-frame relative flex flex-col overflow-hidden bg-abyss font-sans text-ink antialiased">
      {/* neutral spatial field */}
      <div className="ambient" aria-hidden />

      <ViewHarness />
    </div>
  );
}
