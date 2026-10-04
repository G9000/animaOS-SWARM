import { useRef, useState, type FormEvent } from 'react';
import type { MemoryType } from '@animaOS-SWARM/sdk';

import { EntityList, entityKey } from '../components/memory/EntityList';
import { FactList } from '../components/memory/FactList';
import { MemoryList } from '../components/memory/MemoryList';
import { useMemory } from '../hooks/useMemory';
import { COMPANION_UNREACHABLE } from '../lib/approvals';
import {
  ENTITIES_EMPTY,
  FACTS_EMPTY,
  MEMORY_EMPTY,
  MEMORY_SEARCH_EMPTY,
  MEMORY_SORT_LABELS,
  MEMORY_TYPES,
  MEMORY_TYPE_LABELS,
  filterByType,
  groupFacts,
  sortMemories,
  type MemorySort,
} from '../lib/memory';

export interface MemoryPageProps {
  /** The companion. */
  agentId: string;
  online: boolean;
  /** `LiveState.epoch`: a snapshot or resync reads again. */
  epoch: number;
}

const TABS = [
  { id: 'memories', label: 'Memories' },
  { id: 'about', label: 'About you' },
  { id: 'people', label: 'People & things' },
] as const;
type TabId = (typeof TABS)[number]['id'];

const TAB_STORAGE_KEY = 'anima.memory.tab';
const LOADING_TEXT = 'Loading memory…';
const NO_TYPE_MATCH = 'No memories of this type.';

function readTab(): TabId {
  try {
    const stored = sessionStorage.getItem(TAB_STORAGE_KEY);
    if (TABS.some((tab) => tab.id === stored)) return stored as TabId;
  } catch {
    // Storage can be blocked; the page works without it.
  }
  return 'memories';
}

function rememberTab(tab: TabId) {
  try {
    sessionStorage.setItem(TAB_STORAGE_KEY, tab);
  } catch {
    // Not remembered; nothing else depends on it.
  }
}

/** Spec §15.4: what the companion remembers, with edit and delete. Model
 *  text renders only through `RevealedText`. */
export function MemoryPage({ agentId, online, epoch }: MemoryPageProps) {
  const view = useMemory({ agentId, epoch, enabled: online });
  const [tab, setTab] = useState<TabId>(readTab);
  const [searchText, setSearchText] = useState('');
  const [typeFilter, setTypeFilter] = useState<MemoryType | 'all'>('all');
  const [pickedSort, setPickedSort] = useState<MemorySort | null>(null);
  const [removingEntity, setRemovingEntity] = useState<string | null>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);

  const searching = view.query !== '';
  const sort: MemorySort = searching
    ? (pickedSort ?? 'relevance')
    : pickedSort && pickedSort !== 'relevance'
      ? pickedSort
      : 'newest';
  const focusHeading = () => headingRef.current?.focus();

  const selectTab = (next: TabId) => {
    setTab(next);
    rememberTab(next);
  };
  const submitSearch = (event: FormEvent) => {
    event.preventDefault();
    view.search(searchText.trim());
  };

  const entityError =
    tab === 'people' &&
    view.error &&
    removingEntity &&
    view.entities.some((entity) => entityKey(entity) === removingEntity)
      ? { key: removingEntity, message: view.error }
      : null;

  if (!online) {
    return (
      <div className="memory-page">
        <p className="memory-empty" role="status">
          {COMPANION_UNREACHABLE}
        </p>
      </div>
    );
  }

  const shown = sortMemories(filterByType(view.memories, typeFilter), sort);
  const groups = groupFacts(view.facts);

  return (
    <div className="memory-page">
      <div className="memory-header">
        <h2>What your companion remembers</h2>
        {!view.error && (
          <button
            type="button"
            className="studio-tool-button"
            onClick={view.refresh}
          >
            Refresh
          </button>
        )}
      </div>
      {view.error && !entityError && (
        <div className="memory-header">
          <p className="memory-error" role="alert">
            {view.error}
          </p>
          <button
            type="button"
            className="studio-tool-button"
            onClick={view.refresh}
          >
            Refresh
          </button>
        </div>
      )}
      <div className="memory-tabs" role="tablist" aria-label="Memory">
        {TABS.map((item) => (
          <button
            key={item.id}
            type="button"
            role="tab"
            id={`memory-tab-${item.id}`}
            className="memory-tab"
            aria-selected={tab === item.id}
            aria-controls="memory-panel"
            onClick={() => selectTab(item.id)}
          >
            {item.label}
          </button>
        ))}
      </div>
      <div
        id="memory-panel"
        role="tabpanel"
        aria-labelledby={`memory-tab-${tab}`}
        className="memory-section"
      >
        {!view.loaded ? (
          <p className="memory-empty" role="status">
            {LOADING_TEXT}
          </p>
        ) : tab === 'memories' ? (
          <>
            <h3 ref={headingRef} tabIndex={-1}>
              Memories
            </h3>
            <form className="memory-search" role="search" onSubmit={submitSearch}>
              <input
                type="search"
                aria-label="Search memories"
                value={searchText}
                onChange={(event) => setSearchText(event.target.value)}
              />
              <button type="submit" className="studio-tool-button">
                Search
              </button>
              {searching && (
                <button
                  type="button"
                  className="studio-tool-button"
                  onClick={() => {
                    setSearchText('');
                    view.search('');
                  }}
                >
                  Clear
                </button>
              )}
            </form>
            <div className="memory-toolbar">
              <div className="memory-toolbar" role="group" aria-label="Type">
                {(['all', ...MEMORY_TYPES] as const).map((type) => (
                  <button
                    key={type}
                    type="button"
                    className="studio-tool-button"
                    aria-pressed={typeFilter === type}
                    onClick={() => setTypeFilter(type)}
                  >
                    {type === 'all' ? 'All' : MEMORY_TYPE_LABELS[type]}
                  </button>
                ))}
              </div>
              <select
                aria-label="Sort memories"
                value={sort}
                onChange={(event) =>
                  setPickedSort(event.target.value as MemorySort)
                }
              >
                {(searching
                  ? (['relevance', 'newest', 'oldest', 'importance'] as const)
                  : (['newest', 'oldest', 'importance'] as const)
                ).map((option) => (
                  <option key={option} value={option}>
                    {MEMORY_SORT_LABELS[option]}
                  </option>
                ))}
              </select>
            </div>
            {view.memories.length === 0 ? (
              <p className="memory-empty">
                {searching ? MEMORY_SEARCH_EMPTY : MEMORY_EMPTY}
              </p>
            ) : shown.length === 0 ? (
              <p className="memory-empty">{NO_TYPE_MATCH}</p>
            ) : (
              <MemoryList
                memories={shown}
                facts={view.facts}
                onEdit={view.edit}
                onDelete={view.remove}
                onTrace={view.trace}
                onDone={focusHeading}
              />
            )}
          </>
        ) : tab === 'about' ? (
          <>
            <h3 ref={headingRef} tabIndex={-1}>
              About you
            </h3>
            <label className="memory-meta">
              <input
                type="checkbox"
                checked={view.includeReplaced}
                onChange={(event) =>
                  view.setIncludeReplaced(event.target.checked)
                }
              />{' '}
              Show replaced facts
            </label>
            {view.facts.length === 0 ? (
              <p className="memory-empty">{FACTS_EMPTY}</p>
            ) : (
              <>
                {groups.preferences.length > 0 && (
                  <section className="memory-group" aria-label="Preferences">
                    <h4>Preferences</h4>
                    <FactList
                      facts={groups.preferences}
                      onReplace={view.replaceFact}
                      onForget={view.removeFact}
                      onDone={focusHeading}
                    />
                  </section>
                )}
                {groups.about.length > 0 && (
                  <section
                    className="memory-group"
                    aria-label="Things you’ve told me"
                  >
                    <h4>Things you’ve told me</h4>
                    <FactList
                      facts={groups.about}
                      onReplace={view.replaceFact}
                      onForget={view.removeFact}
                      onDone={focusHeading}
                    />
                  </section>
                )}
              </>
            )}
          </>
        ) : (
          <>
            <h3 ref={headingRef} tabIndex={-1}>
              People &amp; things
            </h3>
            {view.entities.length === 0 && view.relationships.length === 0 ? (
              <p className="memory-empty">{ENTITIES_EMPTY}</p>
            ) : (
              <EntityList
                entities={view.entities}
                relationships={view.relationships}
                onRemove={view.removeEntity}
                onRemoving={(entity) => setRemovingEntity(entityKey(entity))}
                error={entityError}
                onDone={focusHeading}
              />
            )}
          </>
        )}
      </div>
    </div>
  );
}
