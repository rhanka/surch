import { parse } from 'csv-parse/sync';
import type { BanDocument } from '$lib/types';
import { MAX_EXTERNAL_BAN_ROWS } from './schema';

type CsvRow = Record<string, string | undefined>;

export async function parseBanCsvText(
  csv: string,
  options: { sourceName: string; limit?: number }
): Promise<BanDocument[]> {
  const limit = clampLimit(options.limit);
  const rows = parse(csv, {
    columns: true,
    delimiter: detectDelimiter(csv),
    skip_empty_lines: true,
    trim: true
  }) as CsvRow[];

  const documents: BanDocument[] = [];
  for (const row of rows) {
    const document = rowToDocument(row);
    if (document) {
      documents.push(document);
    }

    if (documents.length >= limit) {
      break;
    }
  }

  return documents;
}

function detectDelimiter(csv: string): ',' | ';' {
  const firstLine = csv.split(/\r?\n/, 1)[0] ?? '';
  return firstLine.includes(';') ? ';' : ',';
}

function rowToDocument(row: CsvRow): BanDocument | null {
  const id = stringField(row, 'id');
  const streetName = stringField(row, 'nom_voie');
  const postcode = stringField(row, 'code_postal');
  const cityCode = stringField(row, 'code_insee');
  const cityName = stringField(row, 'nom_commune');
  const lon = numberField(row, 'lon');
  const lat = numberField(row, 'lat');

  if (!id || !streetName || !postcode || !cityCode || !cityName || lon === null || lat === null) {
    return null;
  }

  const houseNumber = `${stringField(row, 'numero')}${stringField(row, 'rep')}`.trim();
  const labelParts = [houseNumber, streetName, postcode, cityName].filter(Boolean);

  return {
    city_code: cityCode,
    city_name: cityName,
    house_number: houseNumber,
    id,
    label: labelParts.join(' '),
    location: {
      lat,
      lon
    },
    postcode,
    source: 'BAN',
    street_name: streetName
  };
}

function clampLimit(limit: number | undefined): number {
  if (!limit || !Number.isFinite(limit) || limit <= 0) {
    return MAX_EXTERNAL_BAN_ROWS;
  }

  return Math.min(Math.trunc(limit), MAX_EXTERNAL_BAN_ROWS);
}

function stringField(row: CsvRow, key: string): string {
  return row[key]?.trim() ?? '';
}

function numberField(row: CsvRow, key: string): number | null {
  const value = Number(row[key]);
  return Number.isFinite(value) ? value : null;
}
