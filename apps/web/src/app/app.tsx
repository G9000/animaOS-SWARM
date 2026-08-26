import { ViewHarness } from '../ViewHarness';

export function App() {
  return (
    <div className="relative flex h-screen flex-col overflow-hidden bg-abyss font-sans text-ink antialiased">
      {/* ambient aurora background */}
      <div className="ambient" aria-hidden />

      <ViewHarness />
    </div>
  );
}
