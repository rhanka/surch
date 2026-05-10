<script lang="ts">
  import { emptySuggestionMessage } from '$lib/banUiState';
  import type { BanDocument } from '$lib/types';

  type Props = {
    suggestions?: BanDocument[];
    selected?: BanDocument | null;
    query?: string;
    isLoading?: boolean;
    onQueryChange?: (query: string) => void;
    onSelect?: (document: BanDocument) => void;
  };

  let {
    suggestions = [],
    selected = null,
    query = '',
    isLoading = false,
    onQueryChange = () => {},
    onSelect = () => {}
  }: Props = $props();

  const emptyMessage = $derived(emptySuggestionMessage(query, isLoading));
</script>

<section class="autocomplete" aria-labelledby="address-search-title">
  <div class="header">
    <div>
      <p>Recherche adresse</p>
      <h2 id="address-search-title">Autocomplete BAN</h2>
    </div>
    {#if isLoading}
      <span>recherche</span>
    {/if}
  </div>

  <label>
    <span>Adresse</span>
    <input
      type="search"
      autocomplete="off"
      placeholder="Rue de Rivoli, Bordeaux, Strasbourg..."
      value={query}
      oninput={(event) => onQueryChange((event.currentTarget as HTMLInputElement).value)}
    />
  </label>

  <div class="suggestions" role="listbox" aria-label="Suggestions BAN">
    {#each suggestions as suggestion}
      <button
        type="button"
        class:selected={selected?.id === suggestion.id}
        onclick={() => onSelect(suggestion)}
      >
        <strong>{suggestion.label}</strong>
        <span>{suggestion.postcode} {suggestion.city_name}</span>
      </button>
    {:else}
      <p class="empty">{emptyMessage}</p>
    {/each}
  </div>
</section>

<style>
  .autocomplete {
    display: grid;
    gap: 14px;
  }

  .header {
    display: flex;
    align-items: start;
    justify-content: space-between;
    gap: 16px;
  }

  .header p,
  label span,
  .suggestions span,
  .empty {
    color: #5b6472;
    font-size: 0.86rem;
  }

  .header p,
  h2,
  .empty {
    margin: 0;
  }

  h2 {
    color: #17202a;
    font-size: 1.05rem;
    line-height: 1.25;
  }

  .header > span {
    border: 1px solid #cfd5dd;
    border-radius: 999px;
    color: #3d4754;
    font-size: 0.78rem;
    padding: 3px 8px;
  }

  label {
    display: grid;
    gap: 8px;
  }

  input {
    border: 1px solid #c7ced8;
    border-radius: 8px;
    color: #17202a;
    font: inherit;
    min-height: 44px;
    padding: 0 12px;
  }

  input:focus {
    border-color: #1f5d50;
    outline: 2px solid rgb(31 93 80 / 18%);
  }

  .suggestions {
    display: grid;
    gap: 8px;
  }

  .suggestions button {
    background: #ffffff;
    border: 1px solid #d9dee6;
    border-radius: 8px;
    color: #17202a;
    cursor: pointer;
    display: grid;
    font: inherit;
    gap: 4px;
    padding: 11px 12px;
    text-align: left;
  }

  .suggestions button.selected {
    border-color: #1f5d50;
    box-shadow: 0 0 0 2px rgb(31 93 80 / 13%);
  }
</style>
