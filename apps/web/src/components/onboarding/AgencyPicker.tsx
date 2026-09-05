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
              1 manager + {template.members.length - 1} specialists · {template.starter.title}
            </span>
          </button>
        ))}
      </div>
      <div className="grid gap-2 sm:grid-cols-2">
        {[
          [
            'generate',
            'Generate my agency',
            'Describe your goals and let AI propose your team.',
          ],
          [
            'scratch',
            'Start from scratch',
            'Start with your workspace manager and add specialists later.',
          ],
        ].map(([id, title, description]) => (
          <button
            key={id}
            type="button"
            aria-pressed={selected === id}
            onClick={() => onSelect(id)}
            className={`rounded-xl border p-3 text-left transition hover:border-accent/60 ${selected === id ? 'border-accent bg-accent/[0.08]' : 'border-line'}`}
          >
            <span className="block text-sm font-medium text-ink">{title}</span>
            <span className="mt-1 block text-xs text-ink-3">{description}</span>
          </button>
        ))}
      </div>
    </section>
  );
}
