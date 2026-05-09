<script lang="ts">
  type EngineCard = {
    label: string;
    status: 'idle' | 'ok' | 'error';
    latencyMs?: number;
    topHit?: string | null;
    total?: number | null;
    error?: string;
  };

  type Props = {
    surch?: EngineCard;
    opensearch?: EngineCard;
    overlap?: number | null;
    raw?: unknown;
  };

  let {
    surch = { label: 'Surch', status: 'idle' },
    opensearch = { label: 'OpenSearch', status: 'idle' },
    overlap = null,
    raw = null
  }: Props = $props();

  const rawJson = $derived(raw ? JSON.stringify(raw, null, 2) : '');
</script>

<section class="comparison" aria-labelledby="comparison-title">
  <div class="header">
    <div>
      <p>Comparaison</p>
      <h2 id="comparison-title">Surch vs OpenSearch</h2>
    </div>
    {#if overlap !== null}
      <span>{overlap} ID commun(s)</span>
    {/if}
  </div>

  <div class="cards">
    <article class:error={surch.status === 'error'}>
      <h3>{surch.label}</h3>
      <dl>
        <div>
          <dt>Status</dt>
          <dd>{surch.status}</dd>
        </div>
        <div>
          <dt>Top hit</dt>
          <dd>{surch.topHit ?? 'n/a'}</dd>
        </div>
        <div>
          <dt>Total</dt>
          <dd>{surch.total ?? 'n/a'}</dd>
        </div>
        <div>
          <dt>Latence</dt>
          <dd>{surch.latencyMs !== undefined ? `${surch.latencyMs} ms` : 'n/a'}</dd>
        </div>
      </dl>
      {#if surch.error}
        <p class="engine-error">{surch.error}</p>
      {/if}
    </article>

    <article class:error={opensearch.status === 'error'}>
      <h3>{opensearch.label}</h3>
      <dl>
        <div>
          <dt>Status</dt>
          <dd>{opensearch.status}</dd>
        </div>
        <div>
          <dt>Top hit</dt>
          <dd>{opensearch.topHit ?? 'n/a'}</dd>
        </div>
        <div>
          <dt>Total</dt>
          <dd>{opensearch.total ?? 'n/a'}</dd>
        </div>
        <div>
          <dt>Latence</dt>
          <dd>{opensearch.latencyMs !== undefined ? `${opensearch.latencyMs} ms` : 'n/a'}</dd>
        </div>
      </dl>
      {#if opensearch.error}
        <p class="engine-error">{opensearch.error}</p>
      {/if}
    </article>
  </div>

  {#if rawJson}
    <details>
      <summary>Réponse brute</summary>
      <pre>{rawJson}</pre>
    </details>
  {/if}
</section>

<style>
  .comparison {
    display: grid;
    gap: 14px;
  }

  .header,
  .cards,
  dl div {
    display: grid;
    gap: 10px;
  }

  .header {
    align-items: start;
    grid-template-columns: minmax(0, 1fr) auto;
  }

  .header p,
  dt,
  .header span {
    color: #5b6472;
    font-size: 0.84rem;
  }

  .header p,
  h2,
  h3,
  dl,
  dd,
  .engine-error {
    margin: 0;
  }

  h2 {
    font-size: 1.05rem;
  }

  h3 {
    font-size: 1rem;
  }

  .cards {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  article {
    border: 1px solid #d9dee6;
    border-radius: 8px;
    display: grid;
    gap: 10px;
    padding: 12px;
  }

  article.error {
    background: #fff8f8;
    border-color: #efb9b9;
  }

  dl {
    display: grid;
    gap: 8px;
  }

  dl div {
    grid-template-columns: 82px minmax(0, 1fr);
  }

  dd {
    overflow-wrap: anywhere;
  }

  .engine-error {
    color: #9b1c1c;
    font-size: 0.86rem;
    overflow-wrap: anywhere;
  }

  details {
    border: 1px solid #e1e5ea;
    border-radius: 8px;
    padding: 10px 12px;
  }

  summary {
    cursor: pointer;
  }

  pre {
    background: #f8fafb;
    margin: 10px 0 0;
    max-height: 260px;
    overflow: auto;
    overflow-wrap: anywhere;
    padding: 10px;
    white-space: pre-wrap;
  }

  @media (max-width: 760px) {
    .cards {
      grid-template-columns: 1fr;
    }
  }
</style>
