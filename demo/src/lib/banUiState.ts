import type { BanDocument } from './types';

export function shouldRequestSuggestions(query: string): boolean {
  return query.trim().length >= 2;
}

export function extractSuggestionDocuments(response: unknown): BanDocument[] {
  const record = asRecord(response);
  const candidates = record?.suggestions ?? record?.documents ?? record?.hits;
  return Array.isArray(candidates) ? (candidates as BanDocument[]).slice(0, 8) : [];
}

export function datasetStatus(response: unknown): string {
  const record = asRecord(response);
  const dataset = asRecord(record?.summary) ?? asRecord(record?.dataset) ?? record;
  const count = numberValue(dataset?.documentCount ?? dataset?.documents);
  const name = typeof dataset?.name === 'string' ? dataset.name : 'BAN';

  return count !== undefined ? `${name}: ${count} adresse(s) prêtes` : `${name}: dataset prêt`;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' ? (value as Record<string, unknown>) : null;
}

function numberValue(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}
