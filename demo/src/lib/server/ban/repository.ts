import { existsSync, readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import type { BanDatasetSummary, BanDocument, BanSourceProfile } from '$lib/types';
import { getBanTinyFixture } from '../fixture';
import { parseBanCsvText } from './csv';
import {
  DEFAULT_BAN_SUGGESTIONS,
  MAX_BAN_SUGGESTIONS,
  MAX_EXTERNAL_BAN_ROWS,
  type BanRepositoryEnv
} from './schema';

export type SuggestRequest = {
  query: string;
  limit?: number;
};

export type BanRepository = {
  summary: () => BanDatasetSummary;
  sourceProfile: () => BanSourceProfile;
  documents: () => BanDocument[];
  suggest: (request: SuggestRequest) => BanDocument[];
  hydrate: (ids: string[]) => BanDocument[];
};

export async function createBanRepository(
  env: BanRepositoryEnv = process.env
): Promise<BanRepository> {
  const csvPath = env.BAN_CSV_PATH;
  if (!csvPath) {
    return createTinyRepository();
  }

  if (!existsSync(csvPath)) {
    throw new Error(`BAN_CSV_PATH does not exist: ${csvPath}`);
  }

  const limit = parseLimit(env.BAN_SAMPLE_LIMIT);
  const raw = readFileSync(csvPath);
  const csv = csvPath.endsWith('.gz') ? gunzipSync(raw).toString('utf8') : raw.toString('utf8');
  const documents = await parseBanCsvText(csv, { sourceName: csvPath, limit });

  return createInMemoryBanRepository({
    documents,
    source: {
      kind: 'csv',
      name: fileName(csvPath),
      offline: false,
      bounded: documents.length >= limit,
      path: csvPath
    }
  });
}

export function createInMemoryBanRepository(input: {
  documents: BanDocument[];
  source: BanSourceProfile;
}): BanRepository {
  const byId = new Map(input.documents.map((document) => [document.id, document]));

  return {
    summary: () => ({
      name: input.source.name,
      documentCount: input.documents.length
    }),
    sourceProfile: () => input.source,
    documents: () => input.documents,
    suggest: ({ query, limit }) => {
      const normalized = normalize(query);
      if (normalized.length < 2) {
        return [];
      }

      const max = clampSuggestionLimit(limit);
      return input.documents
        .filter((document) => searchableText(document).includes(normalized))
        .slice(0, max);
    },
    hydrate: (ids) =>
      ids.map((id) => byId.get(id)).filter((document): document is BanDocument => Boolean(document))
  };
}

function createTinyRepository(): BanRepository {
  const fixture = getBanTinyFixture();
  return createInMemoryBanRepository({
    documents: fixture.documents,
    source: {
      kind: 'tiny',
      name: fixture.name,
      offline: true,
      bounded: false
    }
  });
}

function searchableText(document: BanDocument): string {
  return normalize(
    `${document.label} ${document.street_name} ${document.city_name} ${document.postcode} ${document.id}`
  );
}

function normalize(value: string): string {
  return value
    .normalize('NFD')
    .replace(/\p{Diacritic}/gu, '')
    .toLowerCase();
}

function clampSuggestionLimit(limit: number | undefined): number {
  if (!limit || !Number.isFinite(limit) || limit <= 0) {
    return DEFAULT_BAN_SUGGESTIONS;
  }

  return Math.min(Math.trunc(limit), MAX_BAN_SUGGESTIONS);
}

function parseLimit(value: string | undefined): number {
  if (!value) {
    return MAX_EXTERNAL_BAN_ROWS;
  }

  const parsed = Number(value);
  return Number.isFinite(parsed) && parsed > 0
    ? Math.min(Math.trunc(parsed), MAX_EXTERNAL_BAN_ROWS)
    : MAX_EXTERNAL_BAN_ROWS;
}

function fileName(path: string): string {
  return path.split('/').filter(Boolean).at(-1) ?? 'ban.csv';
}
