import { json } from '@sveltejs/kit';
import { getBanLoadPayload } from '$lib/server/ban/bulk';
import { createBanRepository } from '$lib/server/ban/repository';
import {
  BAN_ACTIVE_INDEX,
  loadBanDocuments,
  parseEngineId
} from '$lib/server/engines';
import { toDemoErrorBody, type DemoEngineErrorBody } from '$lib/server/demoErrors';
import type { EngineId } from '$lib/types';
import type { RequestHandler } from './$types';

type EngineLoadResult =
  | Awaited<ReturnType<typeof loadBanDocuments>>
  | DemoEngineErrorBody;

export const POST: RequestHandler = async ({ request, fetch }) => {
  const body = await request.json().catch(() => ({}));
  const engines = parseRequestedEngines(asRecord(body)?.engines);
  const repository = await createBanRepository();
  const summary = repository.summary();
  const source = repository.sourceProfile();
  const documents = repository.documents();
  const payload = getBanLoadPayload({
    datasetName: summary.name,
    indexName: BAN_ACTIVE_INDEX,
    documents
  });

  const loaded = await Promise.allSettled(
    engines.map(async (engine) => [
      engine,
      await loadBanDocuments(engine, BAN_ACTIVE_INDEX, documents, fetch)
    ] as const)
  );
  const engineResults = Object.fromEntries(
    loaded.map((result, index) => {
      const engine = engines[index];
      return [
        engine,
        result.status === 'fulfilled'
          ? result.value[1]
          : engineLoadError(engine, result.reason)
      ];
    })
  ) as Record<EngineId, EngineLoadResult>;

  return json({
    summary: {
      ...summary,
      indexName: BAN_ACTIVE_INDEX
    },
    source,
    index: BAN_ACTIVE_INDEX,
    bulk: {
      path: payload.bulk.path,
      contentType: payload.bulk.contentType,
      documentCount: payload.bulk.documentCount
    },
    engines: engineResults,
    operations: engineResults,
    partial: loaded.some((result) => result.status === 'rejected')
  });
};

function engineLoadError(engine: EngineId, error: unknown): DemoEngineErrorBody {
  const formatted = toDemoErrorBody(error);
  if (formatted) {
    return formatted;
  }

  return {
    error: 'demo_upstream_error',
    status: 502,
    engine,
    path: `/${BAN_ACTIVE_INDEX}`,
    message: error instanceof Error ? error.message : String(error)
  };
}

function parseRequestedEngines(value: unknown): EngineId[] {
  if (value === undefined) {
    return ['surch', 'opensearch'];
  }

  if (!Array.isArray(value)) {
    throw new Error('engines must be an array');
  }

  return [...new Set(value.map(parseEngineId))];
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}
