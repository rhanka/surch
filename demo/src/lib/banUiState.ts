import type { BanDocument } from './types';

const COMPARE_SELECTION_ERROR = 'Sélectionne une adresse avant de comparer.';

export type BanDatasetUiState = {
  documentCount?: number;
  isActiveDataset: boolean;
  name: string;
  sourceLabel: string;
  status: string;
  usesTinyFallback: boolean;
};

export function shouldRequestSuggestions(query: string): boolean {
  return query.trim().length >= 2;
}

export function shouldAutoLoadBanDataset(response: unknown): boolean {
  return banDatasetUiState(response).isActiveDataset;
}

export function extractSuggestionDocuments(response: unknown): BanDocument[] {
  const record = asRecord(response);
  const candidates = record?.suggestions ?? record?.documents ?? record?.hits;
  return Array.isArray(candidates) ? (candidates as BanDocument[]).slice(0, 8) : [];
}

export function datasetStatus(response: unknown): string {
  return banDatasetUiState(response).status;
}

export function canCompareAddress(document: BanDocument | null | undefined): document is BanDocument {
  if (!document) {
    return false;
  }

  return document.id.trim().length > 0 && document.label.trim().length > 0;
}

export function compareAddressError(document: BanDocument | null | undefined): string | null {
  return canCompareAddress(document) ? null : COMPARE_SELECTION_ERROR;
}

export function banDatasetUiState(response: unknown): BanDatasetUiState {
  const record = asRecord(response);
  const dataset = asRecord(record?.summary) ?? asRecord(record?.dataset) ?? record;
  const source = asRecord(record?.source);
  const count = numberValue(dataset?.documentCount ?? dataset?.documents);
  const name = typeof dataset?.name === 'string' ? dataset.name : 'BAN';
  const sourceKind = typeof source?.kind === 'string' ? source.kind : '';
  const usesTinyFallback = sourceKind === 'tiny' || name === 'ban_tiny';
  const state: BanDatasetUiState = {
    isActiveDataset: sourceKind === 'csv' && !usesTinyFallback,
    name,
    sourceLabel: sourceLabel(name, sourceKind, usesTinyFallback),
    status: count !== undefined ? `${name}: ${count} adresse(s) prêtes` : `${name}: dataset prêt`,
    usesTinyFallback
  };

  if (count !== undefined) {
    state.documentCount = count;
  }

  return state;
}

function sourceLabel(name: string, sourceKind: string, usesTinyFallback: boolean): string {
  if (usesTinyFallback) {
    return `Fallback local: ${name}`;
  }

  if (sourceKind === 'csv') {
    return `Dataset actif: ${name}`;
  }

  return `Source BAN: ${name}`;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' ? (value as Record<string, unknown>) : null;
}

function numberValue(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}
