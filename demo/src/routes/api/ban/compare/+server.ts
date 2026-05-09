import { json } from '@sveltejs/kit';
import { createBanRepository } from '$lib/server/ban/repository';
import { DEFAULT_BAN_SUGGESTIONS, MAX_BAN_SUGGESTIONS } from '$lib/server/ban/schema';
import {
  BAN_ACTIVE_INDEX,
  compareBanIndexSearch
} from '$lib/server/engines';
import { toDemoErrorResponse } from '$lib/server/demoErrors';
import type { EngineResponse } from '$lib/types';
import type { RequestHandler } from './$types';

export const POST: RequestHandler = async ({ request, fetch }) => {
  const body = asRecord(await request.json().catch(() => ({})));
  const query = parseQuery(body?.query);
  const expectedId = parseOptionalString(body?.id);
  const limit = clampLimit(body?.limit);
  const repository = await createBanRepository();
  const summary = repository.summary();
  const source = repository.sourceProfile();
  const searchBody = {
    query: {
      match: {
        label: query
      }
    },
    size: limit
  };
  const queryDescriptor = {
    id: 'active_label_match',
    label: `Match label: ${query}`,
    kind: 'search',
    expectedId,
    body: searchBody
  };

  try {
    const result = await compareBanIndexSearch(BAN_ACTIVE_INDEX, searchBody, fetch);

    return json({
      query: queryDescriptor,
      summary: {
        ...summary,
        indexName: BAN_ACTIVE_INDEX
      },
      source,
      ...result,
      overlap:
        'response' in result.opensearch
          ? overlapFromResponses(result.surch, result.opensearch)
          : null,
      guardrails: activeCompareGuardrails()
    });
  } catch (error) {
    const formatted = toDemoErrorResponse(error);

    if (formatted) {
      return json(formatted.body, { status: formatted.status });
    }

    throw error;
  }
};

function parseQuery(value: unknown): string {
  if (typeof value !== 'string' || value.trim().length === 0) {
    throw new Error('query must be a non-empty string');
  }

  return value.trim();
}

function parseOptionalString(value: unknown): string | null {
  return typeof value === 'string' && value.trim().length > 0 ? value.trim() : null;
}

function clampLimit(value: unknown): number {
  if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) {
    return DEFAULT_BAN_SUGGESTIONS;
  }

  return Math.min(Math.trunc(value), MAX_BAN_SUGGESTIONS);
}

function overlapFromResponses(left: EngineResponse, right: EngineResponse): number | null {
  const leftIds = hitIds(left.response);
  const rightIds = hitIds(right.response);

  if (!leftIds.length || !rightIds.length) {
    return null;
  }

  return leftIds.filter((id) => rightIds.includes(id)).length;
}

function hitIds(response: unknown): string[] {
  const record = asRecord(response);
  const hits = asRecord(record?.hits);
  const hitList = Array.isArray(hits?.hits) ? hits.hits : [];

  return hitList
    .map((hit) => asRecord(hit)?._id)
    .filter((id): id is string => typeof id === 'string');
}

function activeCompareGuardrails(): string[] {
  return [
    'query body is generated server-side from the selected BAN label',
    'search path is restricted to the active BAN dataset index',
    'do not publish a global Surch/OpenSearch ratio until runtime paths are symmetric'
  ];
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}
