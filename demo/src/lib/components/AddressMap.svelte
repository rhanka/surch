<script lang="ts">
  import 'leaflet/dist/leaflet.css';
  import { onDestroy, onMount } from 'svelte';
  import type { BanDocument } from '$lib/types';

  type Props = {
    selected?: BanDocument | null;
    suggestions?: BanDocument[];
  };

  let { selected = null, suggestions = [] }: Props = $props();
  let mapElement: HTMLDivElement;
  let map: import('leaflet').Map | null = null;
  let selectedMarker: import('leaflet').Marker | null = null;
  let suggestionLayer: import('leaflet').LayerGroup | null = null;
  let leaflet: typeof import('leaflet') | null = null;

  const center = $derived<[number, number]>(
    selected ? [selected.location.lat, selected.location.lon] : [46.7111, 1.7191]
  );

  onMount(async () => {
    leaflet = await import('leaflet');
    map = leaflet.map(mapElement, {
      center,
      zoom: selected ? 15 : 5,
      zoomControl: true
    });
    leaflet
      .tileLayer('https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png', {
        attribution: '&copy; OpenStreetMap contributors',
        maxZoom: 19
      })
      .addTo(map);
    suggestionLayer = leaflet.layerGroup().addTo(map);
    renderMarkers();
  });

  onDestroy(() => {
    map?.remove();
    map = null;
    selectedMarker = null;
    suggestionLayer = null;
  });

  $effect(() => {
    selected;
    suggestions;
    renderMarkers();
  });

  function renderMarkers() {
    if (!map || !leaflet || !suggestionLayer) {
      return;
    }

    suggestionLayer.clearLayers();
    const visibleSuggestions = suggestions.slice(0, 8);
    for (const suggestion of visibleSuggestions) {
      leaflet
        .circleMarker([suggestion.location.lat, suggestion.location.lon], {
          color: '#4f6f66',
          fillColor: '#4f6f66',
          fillOpacity: 0.22,
          radius: 6,
          weight: 1
        })
        .bindPopup(suggestion.label)
        .addTo(suggestionLayer);
    }

    if (selected) {
      const latlng: [number, number] = [selected.location.lat, selected.location.lon];
      if (!selectedMarker) {
        selectedMarker = leaflet.marker(latlng).addTo(map);
      }
      selectedMarker.setLatLng(latlng).bindPopup(selected.label);
      map.setView(latlng, 15, { animate: true });
    } else if (selectedMarker) {
      selectedMarker.remove();
      selectedMarker = null;
      map.setView(center, 5);
    }
  }
</script>

<section class="map-panel" aria-label="Carte OpenStreetMap">
  <div bind:this={mapElement} class="map"></div>
  <p>Fond OpenStreetMap. Les points secondaires correspondent aux premières suggestions.</p>
</section>

<style>
  .map-panel {
    display: grid;
    gap: 8px;
    min-height: 360px;
  }

  .map {
    border: 1px solid #d9dee6;
    border-radius: 8px;
    min-height: 330px;
    overflow: hidden;
  }

  p {
    color: #5b6472;
    font-size: 0.82rem;
    margin: 0;
  }

  :global(.leaflet-container) {
    font: inherit;
  }
</style>
