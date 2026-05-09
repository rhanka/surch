import manifest from './fixtures/ban_tiny.json';
import ndjson from './fixtures/ban_tiny.ndjson?raw';
import type { BanDocument, BanFixture, FixtureOperation } from '$lib/types';

export function getBanTinyFixture(): BanFixture {
  const documents = parseBulkDocuments(ndjson);
  const operations = manifest.operations.map(validateOperation);

  return {
    name: 'ban_tiny',
    description: manifest.description,
    operations,
    documents
  };
}

export function getBanTinyBulkNdjson(): string {
  return ndjson;
}

function parseBulkDocuments(source: string): BanDocument[] {
  const lines = source
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean);

  if (lines.length % 2 !== 0) {
    throw new Error('BAN fixture bulk payload must contain action/source pairs');
  }

  const documents: BanDocument[] = [];

  for (let index = 0; index < lines.length; index += 2) {
    const action = parseJsonRecord(lines[index], 'bulk action');
    const document = parseJsonRecord(lines[index + 1], 'bulk document');
    const indexAction = parseRecord(action.index, 'bulk index action');
    const id = indexAction._id;

    if (typeof id !== 'string' || id !== document.id) {
      throw new Error('BAN fixture bulk action id must match document id');
    }

    documents.push(validateBanDocument(document));
  }

  return documents;
}

function validateOperation(operation: unknown): FixtureOperation {
  const value = parseRecord(operation, 'fixture operation');
  const kind = value.kind;
  const path = value.path;
  const expectedStatus = value.expected_status;

  if (kind !== 'create_index' && kind !== 'bulk' && kind !== 'refresh') {
    throw new Error('BAN fixture operation kind is invalid');
  }

  if (!isSafeApiPath(path)) {
    throw new Error('BAN fixture operation path is invalid');
  }

  if (expectedStatus !== 200) {
    throw new Error('BAN fixture operation expected status is invalid');
  }

  if ('body' in value && value.body !== 'ban_tiny.ndjson') {
    throw new Error('BAN fixture operation body is invalid');
  }

  return {
    kind,
    path,
    ...(value.body === 'ban_tiny.ndjson' ? { body: value.body } : {}),
    expected_status: expectedStatus
  };
}

function validateBanDocument(document: Record<string, unknown>): BanDocument {
  const location = parseRecord(document.location, 'BAN location');
  const parsed = {
    city_code: requireString(document.city_code, 'city_code'),
    city_name: requireString(document.city_name, 'city_name'),
    house_number: requireString(document.house_number, 'house_number'),
    id: requireString(document.id, 'id'),
    label: requireString(document.label, 'label'),
    location: {
      lat: requireNumber(location.lat, 'location.lat'),
      lon: requireNumber(location.lon, 'location.lon')
    },
    postcode: requireString(document.postcode, 'postcode'),
    source: requireSource(document.source),
    street_name: requireString(document.street_name, 'street_name')
  };

  if (!/^[0-9A-Z_]+$/.test(parsed.id)) {
    throw new Error('BAN document id is invalid');
  }

  return parsed;
}

function parseJsonRecord(line: string, label: string): Record<string, unknown> {
  try {
    return parseRecord(JSON.parse(line), label);
  } catch (error) {
    if (error instanceof SyntaxError) {
      throw new Error(`Invalid JSON in ${label}`);
    }

    throw error;
  }
}

function parseRecord(value: unknown, label: string): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }

  return value as Record<string, unknown>;
}

function requireString(value: unknown, label: string): string {
  if (typeof value !== 'string' || value.trim() === '') {
    throw new Error(`BAN document ${label} must be a non-empty string`);
  }

  return value;
}

function requireNumber(value: unknown, label: string): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    throw new Error(`BAN document ${label} must be a finite number`);
  }

  return value;
}

function requireSource(value: unknown): 'BAN' {
  if (value !== 'BAN') {
    throw new Error('BAN document source must be BAN');
  }

  return value;
}

function isSafeApiPath(value: unknown): value is string {
  return typeof value === 'string' && /^\/[a-zA-Z0-9_/-]+$/.test(value) && !value.includes('..');
}
