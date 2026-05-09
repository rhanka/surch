import type { EngineId } from '$lib/types';

export type DemoErrorStatus = 502 | 504;

export type DemoEngineErrorBody = {
  error: 'demo_upstream_error';
  status: DemoErrorStatus;
  engine: EngineId;
  path: string;
  message: string;
  upstreamStatus?: number;
  body?: unknown;
};

const MAX_UPSTREAM_BODY_CHARS = 512;

export class DemoEngineError extends Error {
  readonly status: DemoErrorStatus;
  readonly engine: EngineId;
  readonly path: string;
  readonly upstreamStatus?: number;
  readonly body?: unknown;

  constructor(input: {
    status: DemoErrorStatus;
    engine: EngineId;
    path: string;
    message: string;
    upstreamStatus?: number;
    body?: unknown;
  }) {
    super(input.message);
    this.name = 'DemoEngineError';
    this.status = input.status;
    this.engine = input.engine;
    this.path = input.path;
    this.upstreamStatus = input.upstreamStatus;
    this.body = input.body;
  }
}

export function truncateUpstreamBody(body: string): string {
  if (body.length <= MAX_UPSTREAM_BODY_CHARS) {
    return body;
  }

  return `${body.slice(0, MAX_UPSTREAM_BODY_CHARS)}... [truncated]`;
}

export function toDemoErrorBody(error: unknown): DemoEngineErrorBody | null {
  if (!(error instanceof DemoEngineError)) {
    return null;
  }

  return {
    error: 'demo_upstream_error',
    status: error.status,
    engine: error.engine,
    path: error.path,
    message: error.message,
    ...(error.upstreamStatus === undefined ? {} : { upstreamStatus: error.upstreamStatus }),
    ...(error.body === undefined ? {} : { body: error.body })
  };
}

export function toDemoErrorResponse(
  error: unknown
): { status: DemoErrorStatus; body: DemoEngineErrorBody } | null {
  const body = toDemoErrorBody(error);

  if (!body) {
    return null;
  }

  return { status: body.status, body };
}
