<script lang="ts">
  import { onMount } from 'svelte';
  import {
    datasetStatus,
    extractSuggestionDocuments,
    shouldRequestSuggestions
  } from '$lib/banUiState';
  import AddressAutocomplete from '$lib/components/AddressAutocomplete.svelte';
  import AddressMap from '$lib/components/AddressMap.svelte';
  import ComparisonPanel from '$lib/components/ComparisonPanel.svelte';
  import type { BanDocument, BanFixture, EngineConfig } from '$lib/types';

  type PageData = {
    fixture: BanFixture;
    engines: EngineConfig[];
  };

  type EngineCard = {
    label: string;
    status: 'idle' | 'ok' | 'error';
    latencyMs?: number;
    topHit?: string | null;
    total?: number | null;
    error?: string;
  };

  let { data }: { data: PageData } = $props();
  let query = $state('');
  let suggestions = $state<BanDocument[]>([]);
  let selected = $state<BanDocument | null>(null);
  let isSuggesting = $state(false);
  let isLoadingDataset = $state(false);
  let isComparing = $state(false);
  let datasetMessage = $state('Chargement du dataset BAN...');
  let errorMessage = $state('');
  let rawResult = $state<unknown>(null);
  let surchCard = $state<EngineCard>({ label: 'Surch', status: 'idle' });
  let opensearchCard = $state<EngineCard>({ label: 'OpenSearch', status: 'idle' });
  let overlap = $state<number | null>(null);
  let suggestionSerial = 0;

  const surchEngine = $derived(data.engines.find((engine) => engine.id === 'surch'));
  const opensearchEngine = $derived(data.engines.find((engine) => engine.id === 'opensearch'));

  onMount(() => {
    void refreshDatasetSummary();
  });

  async function onQueryChange(value: string) {
    query = value;
    if (selected?.label !== value) {
      selected = null;
    }

    const serial = ++suggestionSerial;

    if (!shouldRequestSuggestions(value)) {
      suggestions = [];
      return;
    }

    isSuggesting = true;
    try {
      const response = await postJson('/api/ban/suggest', { query: value, limit: 8 });
      if (serial === suggestionSerial) {
        suggestions = extractSuggestionDocuments(response);
        errorMessage = '';
      }
    } catch (error) {
      if (serial === suggestionSerial) {
        suggestions = [];
        errorMessage = formatEndpointError('/api/ban/suggest', error);
      }
    } finally {
      if (serial === suggestionSerial) {
        isSuggesting = false;
      }
    }
  }

  function onSelect(document: BanDocument) {
    selected = document;
    query = document.label;
  }

  async function loadDataset() {
    isLoadingDataset = true;
    errorMessage = '';
    rawResult = null;

    try {
      const response = await postJson('/api/ban/load', { engines: ['surch', 'opensearch'] });
      datasetMessage = datasetStatus(response);
      applyLoadResponse(response);
    } catch (error) {
      datasetMessage = 'Chargement BAN impossible';
      rawResult = { error };
      errorMessage = formatEndpointError('/api/ban/load', error);
    } finally {
      isLoadingDataset = false;
    }
  }

  function applyLoadResponse(response: unknown) {
    rawResult = response;
    const record = asRecord(response);
    const engines = asRecord(record?.engines ?? record?.operations);
    surchCard = loadEngineCard('Surch', engines?.surch);
    opensearchCard = loadEngineCard('OpenSearch', engines?.opensearch);
    overlap = null;

    if (record?.partial === true) {
      errorMessage = 'Chargement BAN partiel: au moins un moteur est indisponible.';
    }
  }

  async function compareAddress() {
    if (!selected) {
      errorMessage = 'Sélectionne une adresse avant de comparer.';
      return;
    }

    isComparing = true;
    errorMessage = '';
    rawResult = null;
    surchCard = { label: 'Surch', status: 'idle' };
    opensearchCard = { label: 'OpenSearch', status: 'idle' };
    overlap = null;

    try {
      const response = await postJson('/api/ban/compare', {
        id: selected.id,
        query: selected.label,
        limit: 8
      });
      applyCompareResponse(response);
    } catch (error) {
      errorMessage = formatEndpointError('/api/ban/compare', error);
      rawResult = { error };
      surchCard = { label: 'Surch', status: 'error', error: 'Comparaison indisponible' };
      opensearchCard = {
        label: 'OpenSearch',
        status: 'error',
        error: 'Comparaison indisponible'
      };
    } finally {
      isComparing = false;
    }
  }

  async function refreshDatasetSummary() {
    try {
      const response = await getJson('/api/ban/dataset');
      datasetMessage = datasetStatus(response);
    } catch (error) {
      datasetMessage = 'Dataset BAN indisponible';
      errorMessage = formatEndpointError('/api/ban/dataset', error);
    }
  }

  async function getJson(path: string) {
    const response = await fetch(path);
    const parsed = await parseJsonResponse(response);

    if (!response.ok) {
      throw parsed;
    }

    return parsed;
  }

  async function postJson(path: string, body: unknown) {
    const response = await fetch(path, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body)
    });
    const parsed = await parseJsonResponse(response);

    if (!response.ok) {
      throw parsed;
    }

    return parsed;
  }

  async function parseJsonResponse(response: Response) {
    return response.json().catch(() => ({
      error: { type: 'non_json_response', message: `${response.status} ${response.statusText}` }
    }));
  }

  function applyCompareResponse(response: unknown) {
    rawResult = response;
    const record = asRecord(response);
    surchCard = engineCard('Surch', record?.surch);
    opensearchCard = engineCard('OpenSearch', record?.opensearch);
    overlap = typeof record?.overlap === 'number' ? record.overlap : overlapFromHits(record);
  }

  function engineCard(label: string, value: unknown): EngineCard {
    const record = asRecord(value);
    const body = asRecord(record?.response);
    const hits = asRecord(body?.hits);
    const total = asRecord(hits?.total);
    const hitList = Array.isArray(hits?.hits) ? hits.hits : [];
    const firstHit = asRecord(hitList[0]);
    const error = asRecord(record?.error);
    const directError = directErrorMessage(record);

    if (directError || error) {
      return {
        label,
        status: 'error',
        latencyMs: numberValue(record?.latencyMs),
        error: directError ?? String(error?.message ?? error?.reason ?? 'Erreur moteur')
      };
    }

    return {
      label,
      status: record ? 'ok' : 'idle',
      latencyMs: numberValue(record?.latencyMs),
      topHit: typeof firstHit?._id === 'string' ? firstHit._id : null,
      total: numberValue(total?.value)
    };
  }

  function loadEngineCard(label: string, value: unknown): EngineCard {
    const record = asRecord(value);
    const directError = directErrorMessage(record);

    if (directError) {
      return {
        label,
        status: 'error',
        error: directError
      };
    }

    return {
      label,
      status: record ? 'ok' : 'idle'
    };
  }

  function directErrorMessage(record: Record<string, unknown> | null): string | null {
    if (typeof record?.error !== 'string') {
      return null;
    }

    return String(record.message ?? record.error);
  }

  function overlapFromHits(record: Record<string, unknown> | null): number | null {
    const surchIds = hitIds(record?.surch);
    const opensearchIds = hitIds(record?.opensearch);
    if (!surchIds.length || !opensearchIds.length) {
      return null;
    }

    return surchIds.filter((id) => opensearchIds.includes(id)).length;
  }

  function hitIds(value: unknown): string[] {
    const record = asRecord(value);
    const body = asRecord(record?.response);
    const hits = asRecord(body?.hits);
    const hitList = Array.isArray(hits?.hits) ? hits.hits : [];
    return hitList
      .map((hit) => asRecord(hit)?._id)
      .filter((id): id is string => typeof id === 'string');
  }

  function asRecord(value: unknown): Record<string, unknown> | null {
    return value && typeof value === 'object' ? (value as Record<string, unknown>) : null;
  }

  function numberValue(value: unknown): number | undefined {
    return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
  }

  function formatEndpointError(path: string, error: unknown): string {
    return `${path} indisponible. ${formatError(error)}`;
  }

  function formatError(error: unknown): string {
    const record = asRecord(error);
    const nested = asRecord(record?.error);
    const message = nested?.message ?? nested?.reason ?? record?.message;
    return typeof message === 'string' ? message : String(error);
  }
</script>

<svelte:head>
  <title>Surch BAN Demo</title>
  <link rel="icon" href="/favicon.svg" />
  <meta
    name="description"
    content="Demo SvelteKit BAN pour comparer Surch et OpenSearch"
  />
</svelte:head>

<main class="shell">
  <section class="workspace" aria-labelledby="demo-title">
    <header class="topbar">
      <div>
        <p class="eyebrow">BAN autocomplete</p>
        <h1 id="demo-title">Surch Demo</h1>
      </div>
      <div class="engine-strip" aria-label="Moteurs">
        <span>Surch: {surchEngine?.url}</span>
        <span>OpenSearch: {opensearchEngine?.url}</span>
      </div>
    </header>

    <section class="status-band" aria-label="Dataset">
      <div>
        <strong>{datasetMessage}</strong>
        <p>
          `npm run ban:download` récupère `adresses-75.csv.gz` depuis adresse.data.gouv.fr;
          `npm run ban:download:france` récupère la BAN nationale hors repo.
        </p>
      </div>
      <button type="button" onclick={loadDataset} disabled={isLoadingDataset}>
        {isLoadingDataset ? 'Chargement...' : 'Charger BAN'}
      </button>
    </section>

    {#if errorMessage}
      <p class="error">{errorMessage}</p>
    {/if}

    <div class="content-grid">
      <section class="panel">
        <AddressAutocomplete
          {suggestions}
          {selected}
          {query}
          isLoading={isSuggesting}
          onQueryChange={onQueryChange}
          onSelect={onSelect}
        />

        <div class="selected-card">
          <div>
            <p>Adresse sélectionnée</p>
            <h2>{selected?.label ?? 'Aucune adresse'}</h2>
          </div>
          <button type="button" class="primary" onclick={compareAddress} disabled={isComparing}>
            {isComparing ? 'Comparaison...' : 'Comparer'}
          </button>
        </div>

        <ComparisonPanel
          surch={surchCard}
          opensearch={opensearchCard}
          {overlap}
          raw={rawResult}
        />
      </section>

      <section class="panel map-shell">
        <AddressMap {selected} {suggestions} />
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

  .shell {
    box-sizing: border-box;
    min-height: 100vh;
    padding: 28px;
  }

  .workspace {
    margin: 0 auto;
    max-width: 1260px;
  }

  .topbar,
  .status-band,
  .selected-card {
    align-items: center;
    display: flex;
    gap: 18px;
    justify-content: space-between;
  }

  .topbar {
    margin-bottom: 18px;
  }

  .eyebrow,
  .status-band p,
  .selected-card p {
    color: #5b6472;
    font-size: 0.84rem;
    margin: 0;
  }

  .eyebrow {
    font-weight: 700;
    letter-spacing: 0;
    text-transform: uppercase;
  }

  h1,
  h2 {
    margin: 0;
  }

  h1 {
    font-size: 2rem;
    line-height: 1.1;
  }

  h2 {
    font-size: 1.04rem;
    line-height: 1.25;
  }

  .engine-strip {
    display: grid;
    gap: 4px;
    justify-items: end;
  }

  .engine-strip span {
    color: #3d4754;
    font-size: 0.84rem;
    overflow-wrap: anywhere;
  }

  .status-band,
  .panel {
    background: #ffffff;
    border: 1px solid #d9dee6;
    border-radius: 8px;
  }

  .status-band {
    margin-bottom: 14px;
    padding: 14px 16px;
  }

  .content-grid {
    display: grid;
    gap: 18px;
    grid-template-columns: minmax(360px, 0.9fr) minmax(420px, 1.1fr);
  }

  .panel {
    display: grid;
    gap: 18px;
    padding: 18px;
  }

  .map-shell {
    min-height: 430px;
  }

  button {
    background: #ffffff;
    border: 1px solid #cfd5dd;
    border-radius: 8px;
    color: #17202a;
    cursor: pointer;
    min-height: 38px;
    padding: 0 14px;
  }

  button.primary {
    background: #1f5d50;
    border-color: #1f5d50;
    color: #ffffff;
  }

  button:disabled {
    cursor: wait;
    opacity: 0.6;
  }

  .error {
    background: #fff0f0;
    border: 1px solid #f1c7c7;
    border-radius: 8px;
    color: #9b1c1c;
    margin: 0 0 14px;
    overflow-wrap: anywhere;
    padding: 10px 12px;
  }

  @media (max-width: 920px) {
    .shell {
      padding: 18px;
    }

    .topbar,
    .status-band,
    .selected-card {
      align-items: stretch;
      display: grid;
    }

    .engine-strip {
      justify-items: start;
    }

    .content-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
