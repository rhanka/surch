<script lang="ts">
  import type { BanFixture, DemoMode, DemoQuery, EngineConfig, QueryId } from '$lib/types';

  type PageData = {
    fixture: BanFixture;
    engines: EngineConfig[];
    queries: DemoQuery[];
  };

  let { data }: { data: PageData } = $props();
  let mode = $state<DemoMode>('surch');
  let queryId = $state<QueryId>('match_label');
  let result = $state<unknown>(null);
  let errorMessage = $state('');
  let isLoading = $state(false);

  const modeLabels: Record<DemoMode, string> = {
    surch: 'Surch',
    opensearch: 'OpenSearch',
    compare: 'Compare'
  };

  const activeEngines = $derived(
    mode === 'compare' ? data.engines : data.engines.filter((engine) => engine.id === mode)
  );
  const selectedQuery = $derived(
    data.queries.find((query) => query.id === queryId) ?? data.queries[0]
  );
  const queryJson = $derived(JSON.stringify(selectedQuery.body ?? { query: { match_all: {} } }, null, 2));
  const resultJson = $derived(result ? JSON.stringify(result, null, 2) : '');

  async function resetDataset() {
    isLoading = true;
    errorMessage = '';
    result = null;

    try {
      if (mode === 'compare') {
        const responses = await Promise.all([
          postJson('/api/demo/reset', { engine: 'surch' }),
          postJson('/api/demo/reset', { engine: 'opensearch' })
        ]);
        result = { reset: responses };
      } else {
        result = await postJson('/api/demo/reset', { engine: mode });
      }
    } catch (error) {
      errorMessage = error instanceof Error ? error.message : String(error);
    } finally {
      isLoading = false;
    }
  }

  async function runSelectedQuery() {
    isLoading = true;
    errorMessage = '';
    result = null;

    try {
      if (mode === 'compare') {
        result = await postJson('/api/compare', { queryId });
      } else {
        result = await postJson(selectedQuery.kind === 'count' ? '/api/count' : '/api/search', {
          engine: mode,
          queryId
        });
      }
    } catch (error) {
      errorMessage = error instanceof Error ? error.message : String(error);
    } finally {
      isLoading = false;
    }
  }

  async function postJson(path: string, body: unknown) {
    const response = await fetch(path, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body)
    });
    const parsed = await response.json();

    if (!response.ok) {
      throw new Error(JSON.stringify(parsed));
    }

    return parsed;
  }
</script>

<svelte:head>
  <title>Surch BAN Demo</title>
  <link rel="icon" href="/favicon.svg" />
  <meta
    name="description"
    content="Demo SvelteKit BAN tiny pour comparer Surch et OpenSearch"
  />
</svelte:head>

<main class="shell">
  <section class="workspace" aria-labelledby="demo-title">
    <div class="topbar">
      <div>
        <p class="eyebrow">BAN tiny</p>
        <h1 id="demo-title">Surch Demo</h1>
      </div>

      <div class="mode-switch" aria-label="Moteur">
        {#each Object.entries(modeLabels) as [id, label]}
          <button
            type="button"
            class:active={mode === id}
            aria-pressed={mode === id}
            onclick={() => {
              mode = id as DemoMode;
            }}
          >
            {label}
          </button>
        {/each}
      </div>
    </div>

    <div class="content-grid">
      <section class="panel" aria-labelledby="fixture-title">
        <div class="panel-header">
          <h2 id="fixture-title">Fixture</h2>
          <span>{data.fixture.documents.length} documents</span>
        </div>

        <div class="document-list">
          {#each data.fixture.documents as document}
            <article class="document-card">
              <div>
                <h3>{document.label}</h3>
                <p>{document.street_name} · {document.postcode} {document.city_name}</p>
              </div>
              <code>{document.id}</code>
            </article>
          {/each}
        </div>
      </section>

      <section class="panel" aria-labelledby="run-title">
        <div class="panel-header">
          <h2 id="run-title">Run</h2>
          <span>{modeLabels[mode]}</span>
        </div>

        <div class="engine-list">
          {#each activeEngines as engine}
            <div class="engine-row">
              <div>
                <strong>{engine.label}</strong>
                <p>{engine.url}</p>
              </div>
              <span class:configured={engine.configured}>
                {engine.configured ? 'configured' : 'default'}
              </span>
            </div>
          {/each}
        </div>

        <label class="field">
          <span>Query</span>
          <select bind:value={queryId}>
            {#each data.queries as query}
              <option value={query.id}>{query.label}</option>
            {/each}
          </select>
        </label>

        <pre class="query-json">{queryJson}</pre>

        <div class="actions">
          <button type="button" onclick={resetDataset} disabled={isLoading}>Load BAN</button>
          <button type="button" class="primary" onclick={runSelectedQuery} disabled={isLoading}>
            Run
          </button>
        </div>

        {#if errorMessage}
          <p class="error">{errorMessage}</p>
        {/if}

        {#if resultJson}
          <pre class="result-json">{resultJson}</pre>
        {/if}
      </section>
    </div>
  </section>
</main>

<style>
  :global(body) {
    margin: 0;
    background: #f6f7f9;
    color: #17202a;
    font-family:
      Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  }

  button {
    font: inherit;
  }

  select {
    font: inherit;
  }

  .shell {
    min-height: 100vh;
    padding: 32px;
    box-sizing: border-box;
  }

  .workspace {
    max-width: 1180px;
    margin: 0 auto;
  }

  .topbar {
    display: flex;
    align-items: end;
    justify-content: space-between;
    gap: 24px;
    margin-bottom: 24px;
  }

  .eyebrow {
    margin: 0 0 4px;
    color: #5b6472;
    font-size: 0.8rem;
    font-weight: 700;
    letter-spacing: 0;
    text-transform: uppercase;
  }

  h1,
  h2,
  h3,
  p {
    margin: 0;
  }

  h1 {
    font-size: 2rem;
    line-height: 1.1;
  }

  h2 {
    font-size: 1rem;
  }

  h3 {
    font-size: 0.98rem;
  }

  .mode-switch {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    background: #e4e7eb;
    border: 1px solid #cfd5dd;
    border-radius: 8px;
    padding: 4px;
    min-width: 330px;
  }

  .mode-switch button {
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: #3d4754;
    cursor: pointer;
    min-height: 36px;
    padding: 0 12px;
  }

  .mode-switch button.active {
    background: #ffffff;
    color: #0b1117;
    box-shadow: 0 1px 2px rgb(15 23 42 / 12%);
  }

  .content-grid {
    display: grid;
    grid-template-columns: minmax(0, 1.1fr) minmax(320px, 0.9fr);
    gap: 18px;
  }

  .panel {
    background: #ffffff;
    border: 1px solid #d9dee6;
    border-radius: 8px;
    padding: 18px;
  }

  .panel-header {
    display: flex;
    justify-content: space-between;
    gap: 16px;
    align-items: center;
    margin-bottom: 16px;
  }

  .panel-header span,
  .engine-row span {
    color: #5b6472;
    font-size: 0.84rem;
  }

  .document-list,
  .engine-list {
    display: grid;
    gap: 10px;
  }

  .document-card,
  .engine-row {
    border: 1px solid #e1e5ea;
    border-radius: 8px;
    padding: 12px;
  }

  .document-card {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 14px;
    align-items: start;
  }

  .document-card p,
  .engine-row p {
    color: #5b6472;
    font-size: 0.9rem;
    margin-top: 4px;
  }

  code {
    color: #1f5d50;
    background: #e8f3f0;
    border-radius: 6px;
    padding: 3px 6px;
    white-space: nowrap;
  }

  .engine-row {
    display: flex;
    align-items: start;
    justify-content: space-between;
    gap: 14px;
  }

  .engine-row .configured {
    color: #1f5d50;
  }

  .field {
    display: grid;
    gap: 8px;
    margin: 14px 0;
  }

  .field span {
    color: #5b6472;
    font-size: 0.84rem;
    font-weight: 700;
  }

  .field select {
    border: 1px solid #cfd5dd;
    border-radius: 8px;
    min-height: 40px;
    padding: 0 10px;
    background: #ffffff;
    color: #17202a;
  }

  .actions {
    display: flex;
    gap: 10px;
    margin: 14px 0;
    flex-wrap: wrap;
  }

  .actions button {
    border: 1px solid #cfd5dd;
    border-radius: 8px;
    background: #ffffff;
    color: #17202a;
    cursor: pointer;
    min-height: 38px;
    padding: 0 14px;
  }

  .actions .primary {
    background: #1f5d50;
    border-color: #1f5d50;
    color: #ffffff;
  }

  .actions button:disabled {
    cursor: wait;
    opacity: 0.6;
  }

  .query-json,
  .result-json {
    border: 1px solid #e1e5ea;
    border-radius: 8px;
    background: #f8fafb;
    color: #17202a;
    margin: 0;
    max-height: 280px;
    overflow: auto;
    padding: 12px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .result-json {
    max-height: 360px;
  }

  .error {
    color: #9b1c1c;
    background: #fff0f0;
    border: 1px solid #f1c7c7;
    border-radius: 8px;
    padding: 10px;
    overflow-wrap: anywhere;
  }

  @media (max-width: 820px) {
    .shell {
      padding: 18px;
    }

    .topbar,
    .content-grid {
      display: grid;
    }

    .mode-switch {
      min-width: 0;
      width: 100%;
    }

    .content-grid {
      grid-template-columns: 1fr;
    }

    .document-card,
    .engine-row {
      grid-template-columns: 1fr;
    }

    .document-card {
      display: grid;
    }

    .engine-row {
      align-items: start;
      flex-direction: column;
    }

    code {
      white-space: normal;
      overflow-wrap: anywhere;
    }
  }
</style>
