import { demoQueries, getDemoQuery } from '$lib/demoQueries';
import type {
  BanDocument,
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
import { documentsToBulkNdjson } from './ban/bulk';
import { getBanTinyBulkNdjson, getBanTinyFixture } from './fixture';

type FetchLike = typeof fetch;

const REQUEST_TIMEOUT_MS = 2_500;
const BULK_REQUEST_TIMEOUT_MS = 60_000;
const ACTIVE_SEARCH_TIMEOUT_MS = 30_000;
const BULK_CHUNK_SIZE = 1_000;
const INDEX_NAME = 'ban_tiny';
export const BAN_ACTIVE_INDEX = 'ban_addresses';

type JsonObject = Record<string, unknown>;

export type ResetResult = {
  engine: EngineId;
  operations: EngineOperationResult[];
};

export type BanLoadResult = ResetResult & {
  index: string;
};

export type QueryRunRequest = {
  engine: EngineId;
  queryId: QueryId;
};

export type QueryRunResult = EngineResponse & {
  query: DemoQuery;
};

export type BanIndexSearchRequest = {
  engine: EngineId;
  index: string;
  body: JsonObject;
};

export type BanIndexSearchResult = EngineResponse & {
  index: string;
};

export type CompareResult = {
  query: DemoQuery;
  surch: EngineResponse;
  opensearch: EngineResponse | DemoEngineErrorBody;
  guardrails: string[];
  partial: boolean;
};

export type BanIndexSearchCompareResult = {
  index: string;
  surch: EngineResponse;
  opensearch: EngineResponse | DemoEngineErrorBody;
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

export async function loadBanDocuments(
  engine: EngineId,
  index: string,
  documents: BanDocument[],
  fetchImpl: FetchLike = fetch
): Promise<BanLoadResult> {
  const parsedEngine = parseEngineId(engine);
  const parsedIndex = parseIndexName(index);
  const operations: EngineOperationResult[] = [];
  const indexPath = `/${parsedIndex}`;
  const refreshPath = `/${parsedIndex}/_refresh`;

  const deleteResponse = await callEngine(parsedEngine, 'DELETE', indexPath, undefined, fetchImpl);
  operations.push({ path: indexPath, status: deleteResponse.status });

  if (deleteResponse.status !== 200 && deleteResponse.status !== 404) {
    throwUnexpectedStatus(parsedEngine, indexPath, '200 or 404', deleteResponse);
  }

  const createResponse = await callEngine(parsedEngine, 'PUT', indexPath, undefined, fetchImpl);
  operations.push({ path: indexPath, status: createResponse.status });

  if (createResponse.status !== 200) {
    throwUnexpectedStatus(parsedEngine, indexPath, '200', createResponse);
  }

  for (const chunk of documentChunks(documents, BULK_CHUNK_SIZE)) {
    const bulkResponse = await callEngine(
      parsedEngine,
      'POST',
      '/_bulk',
      documentsToBulkNdjson(parsedIndex, chunk),
      fetchImpl,
      { timeoutMs: BULK_REQUEST_TIMEOUT_MS }
    );
    operations.push({ path: '/_bulk', status: bulkResponse.status });

    if (bulkResponse.status !== 200) {
      throwUnexpectedStatus(parsedEngine, '/_bulk', '200', bulkResponse);
    }
  }

  const refreshResponse = await callEngine(parsedEngine, 'POST', refreshPath, undefined, fetchImpl);
  operations.push({ path: refreshPath, status: refreshResponse.status });

  if (refreshResponse.status !== 200) {
    throwUnexpectedStatus(parsedEngine, refreshPath, '200', refreshResponse);
  }

  return { engine: parsedEngine, index: parsedIndex, operations };
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

export async function runBanIndexSearch(
  request: BanIndexSearchRequest,
  fetchImpl: FetchLike = fetch
): Promise<BanIndexSearchResult> {
  const engine = parseEngineId(request.engine);
  const index = parseIndexName(request.index);
  const body = stringifyJsonObject(request.body, 'search body');
  const response = await callEngine(engine, 'POST', `/${index}/_search`, body, fetchImpl, {
    timeoutMs: ACTIVE_SEARCH_TIMEOUT_MS
  });

  return {
    engine,
    index,
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

export async function compareBanIndexSearch(
  index: string,
  body: JsonObject,
  fetchImpl: FetchLike = fetch
): Promise<BanIndexSearchCompareResult> {
  const parsedIndex = parseIndexName(index);
  const [surch, opensearch] = await Promise.allSettled([
    runBanIndexSearch({ engine: 'surch', index: parsedIndex, body }, fetchImpl),
    runBanIndexSearch({ engine: 'opensearch', index: parsedIndex, body }, fetchImpl)
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
      index: parsedIndex,
      surch: toEngineResponse(surch.value),
      opensearch: errorBody,
      partial: true
    };
  }

  return {
    index: parsedIndex,
    surch: toEngineResponse(surch.value),
    opensearch: toEngineResponse(opensearch.value),
    partial: false
  };
}

async function callEngine(
  engine: EngineId,
  method: string,
  path: string,
  body: string | undefined,
  fetchImpl: FetchLike,
  options: { timeoutMs?: number } = {}
): Promise<{ status: number; body: unknown }> {
  if (!isFixedDemoPath(path)) {
    throw new Error('engine path is outside the fixed BAN demo surface');
  }

  const config = loadEngineConfig();
  const baseUrl = engine === 'surch' ? config.surch.url : config.opensearch.url;
  const controller = new AbortController();
  const timeoutMs = options.timeoutMs ?? REQUEST_TIMEOUT_MS;
  const timeout = setTimeout(() => controller.abort(), timeoutMs);

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
          message: `${engine} ${path} timed out after ${timeoutMs}ms`
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
  if (path === '/_bulk') {
    return true;
  }

  return /^\/[a-z0-9][a-z0-9_-]*(?:\/_(?:refresh|count|search))?$/.test(path);
}

function parseIndexName(index: string): string {
  if (/^[a-z0-9][a-z0-9_-]*$/.test(index)) {
    return index;
  }

  throw new Error('engine path is outside the fixed BAN demo surface');
}

function stringifyJsonObject(value: JsonObject, label: string): string {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${label} must be a JSON object`);
  }

  return JSON.stringify(value);
}

function documentChunks(documents: BanDocument[], size: number): BanDocument[][] {
  const chunks: BanDocument[][] = [];
  for (let index = 0; index < documents.length; index += size) {
    chunks.push(documents.slice(index, index + size));
  }

  return chunks;
}

function toEngineResponse(result: EngineResponse): EngineResponse {
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
