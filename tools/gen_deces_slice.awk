BEGIN {
  srand(20260515)
  noms_n = split("MARTIN BERNARD DUBOIS THOMAS ROBERT RICHARD PETIT DURAND LEROY MOREAU SIMON LAURENT LEFEBVRE MICHEL GARCIA DAVID BERTRAND ROUX VINCENT FOURNIER MOREL GIRARD ANDRE LEFEVRE MERCIER DUPONT LAMBERT BONNET FRANCOIS MARTINEZ", noms_a, " ")
  pren_n = split("JEAN MARIE PIERRE LOUIS PAUL HENRI ANDRE MARCEL GEORGES ROBERT ROGER JACQUES MICHEL CLAUDE ALAIN GERARD DANIEL BERNARD CHRISTIAN PHILIPPE RENE FRANCOIS YVES JULIEN JEANNE FRANCOISE MICHELINE SUZANNE GERMAINE MADELEINE", pren_a, " ")
  comm_n = split("PARIS LYON MARSEILLE TOULOUSE NICE NANTES STRASBOURG MONTPELLIER BORDEAUX LILLE RENNES REIMS LEHAVRE SAINT-ETIENNE TOULON GRENOBLE DIJON ANGERS NIMES VILLEURBANNE SAINT-DENIS LEMANS AIX-EN-PROVENCE BREST LIMOGES TOURS AMIENS PERPIGNAN METZ BESANCON", comm_a, " ")
  codeinsee_n = split("75056 69123 13055 31555 06088 44109 67482 34172 33063 59350 35238 51454 76351 42218 83137 38185 21231 49007 30189 69266 93066 72181 13001 29019 87085 37261 80021 66136 57463 25056 75001 69001 13001 31001 06001 44001 75102 69002 13002 31002", codeinsee_a, " ")
  src_n = split("INSEE INSEE INSEE INSEE INSEE-OFFICIEL", src_a, " ")
}
END {
  total = 1000
  for (i = 1; i <= total; i++) {
    id = sprintf("deces_%05d", i)
    nom = noms_a[int(rand() * noms_n) + 1]
    p1 = pren_a[int(rand() * pren_n) + 1]
    p2 = pren_a[int(rand() * pren_n) + 1]
    if (p1 == p2) prenoms = p1
    else prenoms = p1 " " p2
    sexe = (rand() < 0.5) ? "M" : "F"
    # year of birth 1900..1960
    yb = 1900 + int(rand() * 61)
    mb = 1 + int(rand() * 12)
    db = 1 + int(rand() * 28)
    date_naiss = sprintf("%04d%02d%02d", yb, mb, db)
    # year of death yb+30..yb+95, capped 2025
    yd = yb + 30 + int(rand() * 65)
    if (yd > 2025) yd = 2025
    md = 1 + int(rand() * 12)
    dd = 1 + int(rand() * 28)
    date_dec = sprintf("%04d%02d%02d", yd, md, dd)
    cb_idx = int(rand() * comm_n) + 1
    cd_idx = int(rand() * comm_n) + 1
    comm_naiss = comm_a[cb_idx]
    comm_dec = comm_a[cd_idx]
    code_insee = codeinsee_a[cb_idx]
    src = src_a[int(rand() * src_n) + 1]
    src_line = i
    # action header
    printf "{\"index\":{\"_id\":\"%s\",\"_index\":\"deces\"}}\n", id
    # doc
    printf "{\"NOM\":\"%s\",\"PRENOMS\":\"%s\",\"SEXE\":\"%s\",\"DATE_NAISSANCE\":\"%s\",\"COMMUNE_NAISSANCE\":\"%s\",\"CODE_INSEE_NAISSANCE\":\"%s\",\"DATE_DECES\":\"%s\",\"COMMUNE_DECES\":\"%s\",\"SOURCE\":\"%s\",\"SOURCE_LINE\":%d}\n", nom, prenoms, sexe, date_naiss, comm_naiss, code_insee, date_dec, comm_dec, src, src_line
  }
}
