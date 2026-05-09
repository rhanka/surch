import { demoQueries, getDemoQuery } from '$lib/demoQueries';
import type {
  DemoQuery,
  EngineId,
  EngineOperationResult,
  EngineResponse,
  QueryId
} from '$lib/types';
import { loadEngineConfig } from './config';
import {
  DemoEngineError,
  toDemoErrorBody,
  truncateUpstreamBody,
  type DemoEngineErrorBody
} from './demoErrors';
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
  opensearch: EngineResponse | DemoEngineErrorBody;
  guardrails: string[];
  partial: boolean;
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
  const deletePath = `/${INDEX_NAME}`;
  const deleteResponse = await callEngine(engine, 'DELETE', deletePath, undefined, fetchImpl);
  operations.push({ path: deletePath, status: deleteResponse.status });

  if (deleteResponse.status !== 200 && deleteResponse.status !== 404) {
    throwUnexpectedStatus(engine, deletePath, '200 or 404', deleteResponse);
  }

  for (const operation of fixture.operations) {
    const body = operation.body ? getBanTinyBulkNdjson() : undefined;
    const response = await callEngine(engine, operationToMethod(operation.kind), operation.path, body, fetchImpl);
    operations.push({ path: operation.path, status: response.status });

    if (response.status !== operation.expected_status) {
      throwUnexpectedStatus(engine, operation.path, String(operation.expected_status), response);
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
  const [surch, opensearch] = await Promise.allSettled([
    runBanQuery({ engine: 'surch', queryId }, fetchImpl),
    runBanQuery({ engine: 'opensearch', queryId }, fetchImpl)
  ]);

  if (surch.status === 'rejected') {
    throw surch.reason;
  }

  if (opensearch.status === 'rejected') {
    const errorBody = toDemoErrorBody(opensearch.reason);

    if (!errorBody || errorBody.engine !== 'opensearch') {
      throw opensearch.reason;
    }

    return {
      query,
      surch: toEngineResponse(surch.value),
      opensearch: errorBody,
      guardrails: demoGuardrails(),
      partial: true
    };
  }

  return {
    query,
    surch: toEngineResponse(surch.value),
    opensearch: toEngineResponse(opensearch.value),
    guardrails: demoGuardrails(),
    partial: false
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
    let response: Response;
    try {
      response = await fetchImpl(`${baseUrl}${path}`, {
        method,
        body,
        headers: body ? { 'content-type': path === '/_bulk' ? 'application/x-ndjson' : 'application/json' } : undefined,
        signal: controller.signal
      });
    } catch (error) {
      if (isAbortError(error)) {
        throw new DemoEngineError({
          status: 504,
          engine,
          path,
          message: `${engine} ${path} timed out after ${REQUEST_TIMEOUT_MS}ms`
        });
      }

      throw new DemoEngineError({
        status: 502,
        engine,
        path,
        message: `${engine} ${path} request failed: ${errorMessage(error)}`
      });
    }

    let text: string;
    try {
      text = await response.text();
    } catch (error) {
      throw new DemoEngineError({
        status: 502,
        engine,
        path,
        upstreamStatus: response.status,
        message: `${engine} ${path} response body could not be read: ${errorMessage(error)}`
      });
    }
    const parsed = parseEngineBody(engine, path, response.status, text);

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

function parseEngineBody(
  engine: EngineId,
  path: string,
  upstreamStatus: number,
  text: string
): unknown {
  if (!text) {
    return null;
  }

  try {
    return JSON.parse(text);
  } catch {
    throw new DemoEngineError({
      status: 502,
      engine,
      path,
      upstreamStatus,
      message: `${engine} ${path} returned a non-JSON response`,
      body: truncateUpstreamBody(text)
    });
  }
}

function throwUnexpectedStatus(
  engine: EngineId,
  path: string,
  expected: string,
  response: { status: number; body: unknown }
): never {
  throw new DemoEngineError({
    status: 502,
    engine,
    path,
    upstreamStatus: response.status,
    message: `${engine} ${path} expected status ${expected}, got ${response.status}`,
    body: response.body
  });
}

function isAbortError(error: unknown): boolean {
  return (
    error instanceof DOMException && error.name === 'AbortError'
  ) || (error instanceof Error && error.name === 'AbortError');
}

function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message) {
    return error.message;
  }

  return String(error);
}

function demoGuardrails(): string[] {
  return [
    'BAN tiny contains 3 documents.',
    'Do not publish a global Surch/OpenSearch ratio until runtime paths are symmetric.',
    'Oracle validation is required before interpreting timings.'
  ];
}
