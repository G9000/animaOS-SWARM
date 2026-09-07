import { AGENCY_TEMPLATES } from '../../lib/agency-templates';

export function AgencyPicker({
  selected,
  onSelect,
}: {
  selected: string;
  onSelect(id: string): void;
}) {
  return (
    <section aria-labelledby="agency-picker-heading" className="mb-7 space-y-3">
      <div>
        <h2
          id="agency-picker-heading"
          className="font-display text-2xl font-semibold tracking-tight text-ink"
        >
          What will your agency do?
        </h2>
        <p className="mt-1 text-sm leading-relaxed text-ink-2">
          Start with a ready-made team, or describe one of your own. You can
          edit everyone before creating it.
        </p>
      </div>
      <div className="grid gap-3 sm:grid-cols-3">
        {AGENCY_TEMPLATES.map((template) => (
          <button
            key={template.id}
            type="button"
            aria-pressed={selected === template.id}
            onClick={() => onSelect(template.id)}
            className={`min-w-0 rounded-2xl border p-4 text-left transition hover:border-accent/60 focus-visible:outline-2 focus-visible:outline-accent ${selected === template.id ? 'border-accent bg-accent/[0.08]' : 'border-line bg-white/[0.02]'}`}
          >
            <span aria-hidden="true" className="text-2xl text-accent">
              {template.icon}
            </span>
            <span className="mt-3 block text-sm font-semibold text-ink">
              {template.name}
            </span>
            <span className="mt-2 block text-xs leading-relaxed text-ink-2">
              {template.description}
            </span>
            <span className="mt-3 block font-mono text-[10px] text-ink-3">
              1 manager + {template.members.length - 1} specialists ·{' '}
              {template.starter.title}
            </span>
          </button>
        ))}
      </div>
      <button
        type="button"
        aria-pressed={selected === 'generate'}
        onClick={() => onSelect('generate')}
        className={`w-full rounded-xl border p-4 text-left transition hover:border-accent/60 ${selected === 'generate' ? 'border-accent bg-accent/[0.08]' : 'border-line'}`}
      >
        <span className="block text-sm font-medium text-ink">
          Create a custom agency
        </span>
        <span className="mt-1 block text-xs text-ink-3">
          Describe your own goal, then generate or build a team with the number
          of agents you choose.
        </span>
      </button>
      <div className="flex flex-wrap items-center gap-2 text-sm text-ink-3">
        <span>Only need one agent?</span>
        <button
          type="button"
          aria-pressed={selected === 'scratch'}
          onClick={() => onSelect('scratch')}
          className={`rounded-lg px-2 py-1 font-medium underline underline-offset-4 ${selected === 'scratch' ? 'text-accent' : 'text-ink-2'}`}
        >
          Manager only
        </button>
        {selected === 'scratch' && (
          <span className="text-xs">Selected · no specialist team</span>
        )}
      </div>
    </section>
  );
}
