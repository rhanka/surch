import { demoQueries, getDemoQuery } from '$lib/demoQueries';
import type {
  DemoQuery,
  EngineId,
  EngineOperationResult,
  EngineResponse,
  QueryId
} from '$lib/types';
import { loadEngineConfig } from './config';
import { getBanTinyBulkNdjson, getBanTinyFixture } from './fixture';

type FetchLike = typeof fetch;

const REQUEST_TIMEOUT_MS = 2_500;
const INDEX_NAME = 'ban_tiny';

export type ResetResult = {
  engine: EngineId;
  operations: EngineOperationResult[];
};

export type QueryRunRequest = {
  engine: EngineId;
  queryId: QueryId;
};

export type QueryRunResult = EngineResponse & {
  query: DemoQuery;
};

export type CompareResult = {
  query: DemoQuery;
  surch: EngineResponse;
  opensearch: EngineResponse;
  guardrails: string[];
};

export function parseEngineId(value: unknown): EngineId {
  if (value === 'surch' || value === 'opensearch') {
    return value;
  }

  throw new Error('engine must be `surch` or `opensearch`');
}

export function parseQueryId(value: unknown): QueryId {
  if (
    value === 'count' ||
    value === 'match_label' ||
    value === 'bool_address' ||
    value === 'fuzzy_label'
  ) {
    return value;
  }

  throw new Error('queryId must be a known BAN demo query');
}

export function listDemoQueries(): DemoQuery[] {
  return demoQueries;
}

export async function resetBanTiny(
  engine: EngineId,
  fetchImpl: FetchLike = fetch
): Promise<ResetResult> {
  const fixture = getBanTinyFixture();
  const operations: EngineOperationResult[] = [];

  for (const operation of fixture.operations) {
    const body = operation.body ? getBanTinyBulkNdjson() : undefined;
    const response = await callEngine(engine, operationToMethod(operation.kind), operation.path, body, fetchImpl);
    operations.push({ path: operation.path, status: response.status });

    if (response.status !== operation.expected_status) {
      throw new Error(
        `${engine} ${operation.path} expected ${operation.expected_status}, got ${response.status}`
      );
    }
  }

  return { engine, operations };
}

export async function runBanQuery(
  request: QueryRunRequest,
  fetchImpl: FetchLike = fetch
): Promise<QueryRunResult> {
  const engine = parseEngineId(request.engine);
  const query = getDemoQuery(parseQueryId(request.queryId));

  const path = query.kind === 'count' ? `/${INDEX_NAME}/_count` : `/${INDEX_NAME}/_search`;
  const response = await callEngine(
    engine,
    'POST',
    path,
    query.body ? JSON.stringify(query.body) : undefined,
    fetchImpl
  );

  return {
    engine,
    query,
    status: response.status,
    response: response.body
  };
}

export async function compareBanQuery(
  queryId: QueryId,
  fetchImpl: FetchLike = fetch
): Promise<CompareResult> {
  const query = getDemoQuery(parseQueryId(queryId));
  const [surch, opensearch] = await Promise.all([
    runBanQuery({ engine: 'surch', queryId }, fetchImpl),
    runBanQuery({ engine: 'opensearch', queryId }, fetchImpl)
  ]);

  return {
    query,
    surch: toEngineResponse(surch),
    opensearch: toEngineResponse(opensearch),
    guardrails: [
      'BAN tiny contains 3 documents.',
      'Do not publish a global Surch/OpenSearch ratio until runtime paths are symmetric.',
      'Oracle validation is required before interpreting timings.'
    ]
  };
}

async function callEngine(
  engine: EngineId,
  method: string,
  path: string,
  body: string | undefined,
  fetchImpl: FetchLike
): Promise<{ status: number; body: unknown }> {
  if (!isFixedDemoPath(path)) {
    throw new Error('engine path is outside the fixed BAN demo surface');
  }

  const config = loadEngineConfig();
  const baseUrl = engine === 'surch' ? config.surch.url : config.opensearch.url;
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);

  try {
    const response = await fetchImpl(`${baseUrl}${path}`, {
      method,
      body,
      headers: body ? { 'content-type': path === '/_bulk' ? 'application/x-ndjson' : 'application/json' } : undefined,
      signal: controller.signal
    });
    const text = await response.text();
    const parsed = text ? JSON.parse(text) : null;

    return { status: response.status, body: parsed };
  } finally {
    clearTimeout(timeout);
  }
}

function operationToMethod(kind: string): 'PUT' | 'POST' {
  if (kind === 'create_index') {
    return 'PUT';
  }

  return 'POST';
}

function isFixedDemoPath(path: string): boolean {
  return path === '/_bulk' || path === `/${INDEX_NAME}` || path === `/${INDEX_NAME}/_refresh` || path === `/${INDEX_NAME}/_count` || path === `/${INDEX_NAME}/_search`;
}

function toEngineResponse(result: QueryRunResult): EngineResponse {
  return {
    engine: result.engine,
    status: result.status,
    response: result.response
  };
}
