# Highest-resolution terrain data for Western and Southern Europe: UK, Ireland, France, Benelux, Germany (BKG + 16 Länder), Switzerland, Austria (+ 9 Länder), Italy (+ regions), Spain (+ communities), Portugal, microstates, non-governmental

Run date 2026-11-12 (board item i1311). Read-only research for Scenic Roads (personal offline map). Method: every claim carries the URL of a page or API answer that was fetched with a 2xx answer in this run (by me or by ten sub-agents I briefed, each with its own fetch log); search-result snippets were used only to discover URLs. Requests used the User-Agent `scenic-roads/0.1 (personal offline map)`. No data downloads (HEAD, small range reads, metadata, capabilities and index answers only). Before writing this file I checked every URL in it against the union of the fetch logs (see section 6).

Verdict codes: NATIONAL-OPEN = the national source is the best open one; SUBNATIONAL-BETTER = a regional/state/cantonal/city source that is open beats the national one (finer, newer, or the only open one); RESTRICTED = finer data exists but is paid, request-only, login-gated or under a licence that blocks a personal offline map; GLOBAL-ONLY = nothing open beats the global products. A qualifier in brackets says what is partial. "pts/m2" = lidar points per square metre. Vertical datum is given where a source states it.

## 1. Headline

- Open airborne-lidar DTMs at 1 m or finer cover almost every jurisdiction in this scope: all of England (EA, 1 m composite, 2000-2022 flights), Wales (1 m, 2020-23), France (IGN LiDAR HD, 0.5 m, about 94 % of metropolitan 1 km tiles published, flights 2021-Mar 2026), Netherlands (AHN, 0.5 m), Belgium (three regions), Luxembourg (50 cm, 2024), Switzerland (swissALTI3D 0.5 m, new 2024-2029 flights), Austria (BEV 1 m, nationwide), Germany (every Land has an open 1 m DGM), Spain (PNOA-LiDAR, 5 pts/m2 third coverage flown everywhere, 50 cm DTM published for about 14 of 19 regions), Portugal mainland (0.5 m, 10 pts/m2, 2024-25) and the Azores (1 m, 15 pts/m2, 2024).
- Where nothing finer than the global DEMs is open: Jersey, Guernsey and Gibraltar (nothing found), Isle of Man (finer data is sold by Bluesky), Andorra (15 m national MDT under a copyright restriction), San Marino (nothing of its own found) and, for its own data, Monaco and the Vatican (both are covered by neighbours' open tiles: IGN LiDAR HD over Monaco, Italian PST-A over the east of the Vatican).
- Partial coverage only: Scotland (about 10 % of the new 10 pts/m2 national programme released, rest older phases or 50 m), Northern Ireland (10 m country-wide; 1 m only along river basins, Lough Neagh and the coast), Ireland (GSI open lidar by project; the country-wide 2 m/10 m DTM is sold), Italy (the national PST-A lidar is 1 m but only river corridors on 35-90 % of each region's grid, flown 2008-2014; TINITALY 10 m is the complete layer), Malta (1 m from a 2012 flight, WCS).
- Subnational beats national: all 16 German Länder beat the federal open data (BKG gives only 200 m free; its 1 m DGM is paid from EUR 8,000); Tirol, Vorarlberg, Oberösterreich and Burgenland (0.5 m) beat BEV's 1 m; Geneva (0.2 m, 2025, at least 100 pts/m2), Zürich, Luzern and Basel-Landschaft (0.25 m) beat swisstopo's grid; Catalunya (ICGC, at least 8 pts/m2, 2021-23, with 0.5 m, 1 m and 5 m DTMs) beats CNIG; Navarra (own 50 cm DTM from its 2017 and 2024 flights); Euskadi (1 m); in Italy Trento, Bolzano, Friuli-Venezia Giulia, Emilia-Romagna (0.5 m lidar 2022-26, still growing), Valle d'Aosta and the 5 m regional grids; city lidar in France (Strasbourg 2025 at 100 pts/m2, Lyon 2023 at 30 pts/m2).
- National beats pan-European/global DEMs by a wide margin wherever a 0.5-1 m national or regional DTM is open (Copernicus GLO-30 is a 30 m surface model with absolute vertical accuracy under 4 m, 90 %). The Copernicus 10 m instance (EEA-10) cannot be downloaded by the general public at all (section 4.1), so "GLO-10 for EEA countries" is not a fallback for a personal map.
- Licences that block or complicate a personal offline map: BKG DGM1/DOM1 (paid, from EUR 8,000), DGM5/DGM25 (federal users only); Toscana's 2019-21 lidar (public bodies only); Lombardia's regional lidar (public administrations only); Andorra's MDT ("propiedad intelectual exclusiva"); OS Terrain 5 and APGB (not open); Isle of Man and Guernsey commercial data; NGI Belgium (paid DTED); Portugal's DGT LiDAR (CC BY 4.0, but the file links redirect to a free-account login); FABDEM (CC BY-NC-SA 4.0); commercial DEMs and web terrain services (no offline caching). All other sources found are OGL, CC0, CC BY, dl-de/by or dl-de/zero, IODL or Licence Ouverte, which permit offline copies and derivatives with attribution (Balearic IDEIB layers and Sardegna's re-projected PST-A items are share-alike).
- Not verified (section 5): the Kärnten 2022-2029 laser products, Vorarlberg's file download route, several Italian and Spanish live portals that timed out, Isle of Man (WAF-blocked), Gibraltar's geoportal (403), Lisbon/Porto municipal lidar, and the vertical accuracy of IGN LiDAR HD.

## 2. Summary table (one row per jurisdiction)

Columns: jurisdiction | verdict | best source | resolution | year | licence | access | source URL.

### 2.1 United Kingdom, Crown Dependencies, Gibraltar, Ireland

| Jurisdiction | Verdict | Best source | Resolution | Year | Licence | Access | Source URL |
|---|---|---|---|---|---|---|---|
| United Kingdom (GB-wide) | SUBNATIONAL-BETTER (each nation beats it) | OS Terrain 50 (Ordnance Survey OpenData), GB only; OS Terrain 5 is not in the open product list | 50 m grid, RMSE 4 m | version 2026-07 | OS OpenData, personal use included | keyless bulk ZIP (ASCII grid 161.7 MB) | https://api.os.uk/downloads/v1/products/Terrain50/downloads |
| England | SUBNATIONAL-BETTER (full coverage) | Environment Agency National LIDAR Programme and LIDAR Composite DTM/DSM | 1 m (programme at least 1 pt/m2; 25/50 cm tiles where flown); 2 m, 10 m composites | surveys 2000-Apr 2022 (composite built 2022); newest programme flights 2022-Feb 2023 | OGL | 5 km tiles (GeoTIFF/LAZ), WCS with elevation values, OGC API index | https://environment.data.gov.uk/dataset/9ba4d5ac-d596-445a-9056-dae3ddec0178 |
| Scotland | SUBNATIONAL-BETTER (partial, growing) | Scottish Land LiDAR Programme (Scottish Government, Bluesky) + Phases I-VI, Outer Hebrides, Orkney, Coastal 2026 | 10 pts/m2 (NLP), 4 pts/m2 / 50 cm (Phases III-VI), 16 pts/m2 (Orkney) | NLP releases 29 Jan 2026 and 12 Oct 2026 (almost 10 % of Scotland); flights to Jul 2027 | OGL v3 (two HES sets non-commercial) | 10 km tiles in the portal app; metadata zips; no WCS | https://blogs.gov.scot/digital/2026/10/12/scottish-lidar-user-conference-and-new-land-and-coastal-lidar-data-released/ |
| Wales | SUBNATIONAL-BETTER (near-full) | Welsh Government LiDAR 2020-2023 (DataMapWales) | 1 m DTM/DSM (Wales-wide COGs) | 2020-2023, published Sep 2024 | OGL (layer page says "Not Specified") | COG downloads (48.6 GB 32-bit DTM), WFS tile catalogue | https://datamap.gov.wales/maps/6969 |
| Northern Ireland | SUBNATIONAL-BETTER (partial) | OSNI/LPS 10 m DTM (all NI); DfI River Basin LIDAR 1 m; Lough Neagh 2017; DAERA coastal 2021 | 10 m / 1 m (river basins) | 2004-2014, 2017, 2021 | OGL | OpenDataNI CKAN, blob files; DAERA tiles | https://admin.opendatani.gov.uk/api/3/action/package_search?q=lidar&rows=30 |
| Jersey | GLOBAL-ONLY | none open (5 m contours, 3D model; Jersey Heritage 2020 lidar unpublished) | n/a | 2020 flight | Government copyright | none | https://www.gov.je/PlanningBuilding/JerseyMappingService/Pages/Digital.aspx |
| Guernsey (+ Alderney, Sark, Herm) | GLOBAL-ONLY | none open found | n/a | n/a | n/a | none | https://www.digimap.gg/ |
| Isle of Man | RESTRICTED | 2013 island-wide airborne lidar (Bluesky), sold | 0.5/1/2 m DTM/DSM + points | 2013 | commercial | purchase (gov.im blocked) | https://www.geoconnexion.com/news/bluesky-publishes-first-online-3d-maps-of-isle-of-man |
| Gibraltar | GLOBAL-ONLY | none open found | n/a | Dec 2023 heritage drone lidar only | n/a | geoportal 403 | https://www.gibraltar.gov.gi/press-releases/northern-defences-lidar-survey-and-current-works-8552023-9460 |
| Ireland | NATIONAL-OPEN (partial); country-wide DTM RESTRICTED (sold) | Geological Survey Ireland open topographic lidar; Tailte Éireann DTM (sold) | GSI 1 m; Discovery Programme 13-14 cm; Westmeath 25 cm; TII/OPW 2 m | 2015-2021 | CC BY 4.0 (GSI) | viewer download; ArcGIS REST footprints | https://data.gov.ie/api/3/action/package_search?q=lidar&rows=40 |

### 2.2 France, Belgium, Netherlands, Luxembourg

| Jurisdiction | Verdict | Best source | Resolution | Year | Licence | Access | Source URL |
|---|---|---|---|---|---|---|---|
| France (metropolitan) | NATIONAL-OPEN | IGN LiDAR HD (point clouds, MNT/MNS/MNH) | 0.5 m, at least 10 pulses/m2; 520,016 metropolitan 1 km tiles published (about 94 % by tile count) | flights 2021 to Mar 2026; edition to Sep 2026 | Licence Ouverte 2.0 | tile download, WMS-R GeoTIFF with values, WFS index | https://www.data.gouv.fr/api/1/datasets/675871bd6e60265b46ab356e/ |
| France: regions and cities | SUBNATIONAL-BETTER (locally) | Strasbourg 2025 (100 pts/m2), Métropole de Lyon 2023 (30 pts/m2), Bordeaux 2020 (15), CRAIG 2021 (14) | 50 cm MNT | 2020-2025 | Licence Ouverte 2.0 (Bordeaux v1) | open data portals | https://data.strasbourg.eu/api/explore/v2.1/catalog/datasets/nuage_de_points_3d_2025 |
| Corsica | NATIONAL-OPEN | IGN LiDAR HD (9,321 tiles) | 0.5 m, median about 42 M points/km2 | flights May 2021-Sep 2022 | Licence Ouverte 2.0 | as France | https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=IGNF_LIDAR-HD_METADONNEE:metadata&resultType=hits&CQL_FILTER=BBOX%28geom%2C41.3%2C8.4%2C43.1%2C9.7%29 |
| France overseas (Guadeloupe, Martinique, Guyane, La Réunion, Mayotte) | covered elsewhere; IGN named | IGN LiDAR HD (Guadeloupe and La Réunion tiles so far), RGE ALTI 1/5 m | 0.5 m / 1 m | 2025-26 | Licence Ouverte 2.0 | as France | https://www.data.gouv.fr/api/1/datasets/675871bd6e60265b46ab356e/ |
| Belgium: Flanders | SUBNATIONAL-BETTER (regional is the only source) | Digitaal Hoogtemodel Vlaanderen II | 1 m DTM/DSM, 16 pts/m2 | 2013-2015 (DHMV III flights 2026-27, products 2028) | Flemish free-reuse model licence | WCS, download app | https://www.vlaanderen.be/digitaal-vlaanderen/onze-diensten-en-platformen/earth-observation-data-science-eodas/het-digitaal-hoogtemodel/digitaal-hoogtemodel-vlaanderen-ii |
| Belgium: Wallonia | SUBNATIONAL-BETTER | SPW Relief de la Wallonie LiDAR 2021-2022 | MNT 0.5 m, 6.8 pts/m2 planned | 2021-2022 | CC BY 4.0 | province ZIPs, ATOM, ArcGIS REST | https://metawal.wallonie.be/geonetwork/srv/api/records/3ef17388-54cd-4034-baad-d247243123c2 |
| Belgium: Brussels | SUBNATIONAL-BETTER | UrbIS lidar 2021 (DTM, DSM, point cloud) | DTM 0.5 m, 67 pts/m2 | Oct 2021 | CC0 (DTM), CC BY 4.0 (cloud) | Atom feed | https://geobru-geonetwork.irisnet.be/geonetwork/srv/api/records/1d7bd49d-fe83-4388-af85-6f5dc8ec7909 |
| Belgium (national) | GLOBAL-ONLY / RESTRICTED (national level) | NGI DTED level 1, sold | about 1:100,000 | n/a | copyright NGI, EUR 6,771.46 whole country | purchase | https://metadata.vlaanderen.be/srv/api/records/82271D4B-EBDF-4289-BBB1-69C1C96223DB |
| Netherlands | NATIONAL-OPEN | AHN (AHN4/AHN5, first AHN6 part 13 Oct 2025) | DTM/DSM 0.5 m, AHN4 10-14 pts/m2, 5 cm | AHN6 flights 2025-2026 | CC0 (AHN4) / CC BY 4.0 (AHN6) | PDOK ATOM COG, WCS/WMS | https://www.ahn.nl/dataroom |
| Luxembourg | NATIONAL-OPEN | BD-L-Lidar2024 (ACT) | MNT/MNS 50 cm COG, over 30 pts/m2 | Feb-Apr 2024 | CC0 | direct COG (40 GB single file), tiles | https://data.public.lu/api/1/datasets/lidar-2024-releve-3d-du-territoire-luxembourgeois/ |

### 2.3 Germany

| Jurisdiction | Verdict | Best source | Resolution | Year | Licence | Access | Source URL |
|---|---|---|---|---|---|---|---|
| Germany (federal, BKG) | RESTRICTED (DGM1 paid); free federal = 200 m | BKG DGM1 (mosaic of the Länder) | DGM1 1 m (paid, from EUR 8,000); DGM200 200 m free; DGM5/DGM25 federal users only | age per Land 1 to over 15 years | dl-de/by-2-0 for DGM200 only | download for DGM200; DGM1 contract | https://gdz.bkg.bund.de/index.php/default/digitale-geodaten/digitale-gelandemodelle/digitales-gelandemodell-gitterweite-1-m-dgm1.html |
| Baden-Württemberg | SUBNATIONAL-BETTER | LGL DGM025 + DGM1 (ALS_2 2016-21, ALS_3 since 2022); cloud by order | 0.25 m and 1 m open; at least 8 pts/m2 cloud (fee, min EUR 60) | 2016-2021, 2022 onward | dl-de/by-2-0 | portal ZIP, WCS 2.0.1 | https://www.lgl-bw.de/Produkte/3D-Produkte/Digitale-Gelaendemodelle/index.html |
| Bayern | SUBNATIONAL-BETTER | OpenData Bayern DGM1 + open LAZ cloud + DOM20 | 1 m; cloud at least 4 pts/m2 | re-flown 2016-2025 (ten winters) | CC BY 4.0 | tiles, metalink, REST height service | https://geodaten.bayern.de/opengeodata/json/opengeodata_produkte.json |
| Berlin | SUBNATIONAL-BETTER | ATKIS DGM1, ALS point cloud 9.8 pts/m2, DOM1 | 1 m | ALS Feb-Mar 2021; DGM1 updated to Aug 2025 | dl-de/zero-2-0 | ATOM | https://gdi.berlin.de/geonetwork/srv/api/records/f0e8ff09-2887-3446-9c53-81dbc45af03c |
| Brandenburg | SUBNATIONAL-BETTER | LGB DGM 1 m, ALS LAZ (about 5 pts/m2) | 1 m | DGM published May 2026; cloud published Sep 2025 (about 44-60 % of state) | dl-de/by-2-0 | open directories, WCS/WMS/WMTS | https://geobasis-bb.de/lgb/de/geodaten/3d-produkte/gelaendemodell/ |
| Bremen | SUBNATIONAL-BETTER | ATKIS DGM1 / DGM5 / DOM (open since 9 Jun 2024); cloud on request | 1 m, 5 m | ALS 2015/2017; new ALS flown 18-19 Mar 2026 | CC BY 4.0 | GeoPortal / ATOM | https://www.geo.bremen.de/produkte/3d-produkte/hoehenmodelle-12482 |
| Hamburg | SUBNATIONAL-BETTER | Geländemodelle Hamburg DGM (1 m); no open cloud found | 1 m | flights 2010, 2020, 2022; new ALS Jan-Mar 2026 unpublished | dl-de/by-2-0 | CKAN, ZIP 1.4 GB | https://suche.transparenz.hamburg.de/api/3/action/package_show?id=gelaendemodelle-hamburg-dgm2 |
| Hessen | SUBNATIONAL-BETTER | ATKIS DGM1 / DOM1; ALS cloud for a fee | 1 m; cloud at least 4 pts/m2 | Laserscan 2 closed 2021; Laserscan 3 planned to 2026/27 | "ohne Einschränkung oder Bedingung" (open since 1 Feb 2022) | Downloadcenter | https://hvbg.hessen.de/geoinformation/open-data |
| Mecklenburg-Vorpommern | SUBNATIONAL-BETTER | ATKIS DGM1 (ATOM); ALS cloud paid | 1 m; cloud at least 2 pts/m2 | about 10-year cycle | CC BY 4.0 | ATOM, tile search | https://www.geodaten-mv.de/dienste/dgm_atom |
| Niedersachsen | SUBNATIONAL-BETTER (but 75 % of DGM1 older than 9 years) | LGLN DGM1/DOM1 COG; cloud on order | 1 m; cloud at least 4 pts/m2 | per LGLN map: 75.4 % older than 9 years | CC BY 4.0 | STAC, WCS, WMS | https://dgm.stac.lgln.niedersachsen.de/collections |
| Nordrhein-Westfalen | SUBNATIONAL-BETTER | Geobasis NRW DGM1 + 3D-Messdaten LAZ | 1 m; cloud 4 pts/m2 | 2020-2025; 8 pts/m2 from winter 2026/27 | dl-de/zero-2-0 | bulk tiles (cloud 3.5 TB), WCS | https://www.opengeodata.nrw.de/produkte/geobasis/hm/3dm_l_las/3dm_l_las/ |
| Rheinland-Pfalz | SUBNATIONAL-BETTER | LVermGeoRP DGM1 + LPG laser points | 1 m; about 4 pts/m2 | 2022-2026, a quarter of the state per year | dl-de/by-2-0 | geoshop open data | https://lvermgeo.rlp.de/geodaten-geoshop/open-data |
| Saarland | SUBNATIONAL-BETTER | LVGL DGM1 (2025); no open cloud found | 1 m | 2025 | dl-de/by-2-0 | ATOM + public Nextcloud | https://geoportal.saarland.de/app-article/geobasisdatenuebersicht/ |
| Sachsen | SUBNATIONAL-BETTER | GeoSN DGM1/DOM1 + Laserscandaten LAZ | 1 m | flights 2018-2025 | dl-de/by-2-0 | 2x2 km tiles, batch download | https://www.geodaten.sachsen.de/downloadbereich-digitale-hoehenmodelle-4851.html |
| Sachsen-Anhalt | SUBNATIONAL-BETTER | LVermGeo DGM1 / DOM1 / bDOM20; cloud on request | 1 m | tile years 2021-2025; 2026 flown | dl-de/by-2-0 | downloads, WCS | https://www.lvermgeo.sachsen-anhalt.de/de/gdp-open-data.html |
| Schleswig-Holstein | SUBNATIONAL-BETTER (flight years unverified) | LVermGeo SH DGM1 (OpenGBD) | 1 m, 3-4 pts/m2 | not stated | CC BY 4.0 (CKAN) | download client | https://opendata.schleswig-holstein.de/api/3/action/package_search?q=DGM1&rows=10 |
| Thüringen | SUBNATIONAL-BETTER | TLBG DGM1/DOM1 + LAZ (Höhendaten 2020-2025) | 1 m; cloud 4 pts/m2 | 2020-2025 | dl-de/by-2-0 / CC BY 4.0 | ATOM (read via Wayback) | https://web.archive.org/web/2026/https://geoportal.geoportal-th.de/gaialight-th/_apps/dladownload/dl-dhm.html |

### 2.4 Switzerland, Liechtenstein, Austria

| Jurisdiction | Verdict | Best source | Resolution | Year | Licence | Access | Source URL |
|---|---|---|---|---|---|---|---|
| Switzerland (swisstopo) | NATIONAL-OPEN | swissALTI3D; swissSURFACE3D point cloud and raster | 0.5 m / 2 m; cloud min 15, mean 25-40 pts/m2 (from 2024); ±10 cm | 6-year cycle; new campaign 2024-2029 | swisstopo OGD ("(c) swisstopo") | STAC, direct COG tiles | https://www.swisstopo.admin.ch/en/height-model-swissalti3d |
| Swiss cantons | SUBNATIONAL-BETTER in GE, ZH, LU, BL; others NATIONAL-OPEN | Geneva MNT 2025 0.2 m; Zürich DTM 2022 0.25 m; Luzern 2024 0.25 m; Basel-Landschaft 2024 0.25/0.5 m; AG 0.5 m | 0.2-0.25 m | 2022-2025 | opendata.swiss "Open use" (BL terms unread) | COG/ImageServer/GeoTIFF | https://data.geo.admin.ch/api/stac/v0.9/collections/ch.swisstopo.swissalti3d |
| Liechtenstein | NATIONAL-OPEN (via swisstopo) | swissALTI3D / swissSURFACE3D (published 17 Feb 2026); local DTM 2015 | 0.5 m; local 0.5 m, 4 pts/m2 | 2026; 2015 | swisstopo OGD; opendata.swiss | STAC | https://www.swisstopo.admin.ch/en/height-model-swisssurface3d-update-20260217 |
| Austria (federal, BEV) | NATIONAL-OPEN | ALS-DGM Höhenraster (annual mosaic) | 1 m, ±0.5 m | Stichtage 2019-2025 | CC BY 4.0 | 55 tiles of 50 km, no login | https://www.bev.gv.at/Services/Produkte/Digitales-Gelaendehoehenmodell/ALS-Hoehenraster.html |
| Burgenland | SUBNATIONAL-BETTER | Land Burgenland DGM/DOM 0.5 m | 0.5 m | ALS 2019 | CC BY 4.0 AT | municipality download | https://geodaten.bgld.gv.at/de/downloads/hoehenmodelle-orthofotos.html |
| Kärnten | NATIONAL-OPEN (new campaign unverified) | Land Kärnten 5 m DGM; ALS 2022-2029 products not reachable | 5 m open verified | 2006-2015; 2022-2029 | CC BY 4.0 | gis.ktn.gv.at timed out | https://www.data.gv.at/api/hub/search/search?q=K%C3%A4rnten%20Gel%C3%A4ndemodell&limit=300 |
| Niederösterreich | NATIONAL-OPEN | Land NÖ OGD DGM 10 m; finer in login Geoshop | 10 m open | ALS blocks 2011-2022 | CC BY 4.0 | WFS | https://noe.gv.at/noe/Open-Government-Data/Geographie_Planung.html |
| Oberösterreich | SUBNATIONAL-BETTER | DORIS DGM 0.5 m | 0.5 m | latest flight per sheet 2014-2025 | CC BY 4.0 | municipality ZIPs | https://e-gov.ooe.gv.at/at.gv.ooe.intramapgem/dop/downloads/41501/41501_DGM_tif.zip |
| Salzburg | NATIONAL-OPEN (equal) | SAGIS DGM 1 m | 1 m | epoch 3 (2022-24) partly | CC BY 4.0 | municipality download | https://service.salzburg.gv.at/ogd/client/showDetail/aa7b0528-70c1-42c4-a03f-87d2d6a452d7 |
| Steiermark | NATIONAL-OPEN (new 50 cm campaign unverified) | GIS-Steiermark DGM 1 m | 1 m; new campaign 50 cm | 2008-2014; second campaign from 2022 | CC BY 4.0 | WCS, ZIP | https://www.landesentwicklung.steiermark.at/cms/beitrag/12803182/142970647/ |
| Tirol | SUBNATIONAL-BETTER | Land Tirol DGM/DOM 50 cm; ground at least 8 pts/m2 | 0.5 m | second scan 2017-2023 | CC BY 4.0 | sheet-wise, WCS; cloud on request (EUR 86.09/h) | https://tirol.gv.at/sicherheit/geoinformation/geodaten-tiris/laserscandaten |
| Vorarlberg | SUBNATIONAL-BETTER (download route unverified) | VOGIS Geländehöhenmodell 2023 50 cm | 0.5 m | 4th scan 2023 | CC BY 4.0 | WMS found; file route not | https://vogis.cnv.at/geonetwork/srv/api/records/bd8bc975-a1e9-4374-811f-5773c7a84e33/formatters/xml |
| Wien | NATIONAL-OPEN (equal) | MA 41 DGM 1 m | 1 m | tile updated Oct 2026 | CC BY 4.0 | 2.5 km blocks | https://www.wien.gv.at/stadtplanung/geodatenviewer-ogd-daten-herunterladen |

### 2.5 Italy

| Jurisdiction | Verdict | Best source | Resolution | Year | Licence | Access | Source URL |
|---|---|---|---|---|---|---|---|
| Italy (national) | NATIONAL-OPEN (partial lidar) | TINITALY/1.1 (whole country); PST-A lidar DTM/DSM (MASE) | 10 m; PST-A 1 m (2 m in places), under 15 cm | PST-A 2008-2015; TINITALY 2023; new 25 cm national lidar under contract (no release found) | CC BY 4.0 | tile zips, WCS (TINITALY); web app (PST-A) | https://tinitaly.pi.ingv.it/Download_Area1_1.html |
| Piemonte | SUBNATIONAL-BETTER | ICE 2009-2011 DTM 5 | 5 m, ±0.30 m | 2009-2011 | CC BY 4.0 | WCS (live host timed out) | https://web.archive.org/web/2026/https://www.geoportale.piemonte.it/geonetwork/srv/api/records/r_piemon:224de2ac-023e-441c-9ae0-ea493b217a8e |
| Valle d'Aosta | SUBNATIONAL-BETTER | Regional DTM/DSM from lidar | 2 m whole region; 0.5 m Dora (2005, 2022) | 2005, 2008, 2022 | CC BY 4.0 | GeoDownload | https://geoportale.regione.vda.it/download/dtm/ |
| Lombardia | SUBNATIONAL-BETTER (5 m); regional lidar RESTRICTED | DTM 5x5 (ed. 2015) | 5 m | 2015 | CC BY 4.0 | 3.1 GB ZIP, WMS | https://www.geoportale.regione.lombardia.it/en/note-legali |
| Liguria | NATIONAL-OPEN | PST-A 1 m; regional DTM 5 m (ed. 2023) | 1 m / 5 m | 2008; 2023 | CC BY 4.0 | WCS | https://geodati.gov.it/resource/id/r_liguri:D.2056 |
| Trento (Prov. autonoma) | SUBNATIONAL-BETTER | LiDAR PAT 2014 + 2018 | 0.5 m, 10 pts/m2 below 1800 m | 2013-2018 | CC BY 4.0 | open directory (25,201 tiles) | https://geodati.gov.it/resource/id/p_TN:79c6cdf0-c73a-46ba-870c-704bf4e27ac3 |
| Bolzano / Alto Adige (Prov. autonoma) | SUBNATIONAL-BETTER | DTM 0.5 m (settled areas), 2.5 m (province), Adige 0.2 m | 0.5 / 2.5 / 0.2 m | 2004-06, about 2013, 2024-25 | CC0 | WCS | https://geoservices9.civis.bz.it/geoserver/ows?service=WCS&version=2.0.1&request=GetCapabilities |
| Veneto | SUBNATIONAL-BETTER | DTM Lidar 5 m (2024) | 5 m (from over 40 campaigns, mostly 1 m) | about 2004-2021 | IODL 2.0 | download by commune | https://idt2.regione.veneto.it/idt/downloader/download |
| Friuli-Venezia Giulia | SUBNATIONAL-BETTER | Rilievo LIDAR RAFVG 2017-2020 | 0.5 m, 16 pts/m2 | 2017-2020 | IODL 2.0 (regional standard) | Eagle.fvg app | https://geodati.gov.it/resource/id/r_friuve:m10856-cc-i11826 |
| Emilia-Romagna | SUBNATIONAL-BETTER (growing) | DTM 0.5 m RER 2022 / 2023-24 / 2025-26 | 0.5 m, 8 pts/m2, 8 cm | 2022-2026 | CC BY 4.0 | keyless WCS | https://geoportale.regione.emilia-romagna.it/approfondimenti/rer23_24 |
| Toscana | RESTRICTED | Regional lidar 2019-2021 (public bodies only); PST-A open | 1 m, 1.5 pts/m2 | 2019-2021 | public entities only | JS app | https://www502.regione.toscana.it/geoscopio/geometa/html/r_toscan_261ec1b2-bf25-4973-b853-ac08e060ec62.htm |
| Umbria | NATIONAL-OPEN | PST-A 1 m (58 % of grid) | 1 m | 2008-2014 | CC BY 4.0 | web app | https://web.archive.org/web/2026/https://umbriageo.regione.umbria.it/catalogostazioni/catalogo.aspx?cd=149 |
| Marche | NATIONAL-OPEN | PST-A 1 m (67 %); regional DTM 20 m | 1 m / 20 m | 2008-2014 | CC BY 4.0 | web app | https://www.regione.marche.it/Portals/0/Paesaggio_Territorio_Urbanistica/Cartografia/Metadati/Relazione_DTM.pdf |
| Lazio | NATIONAL-OPEN | PST-A 1 m (36 %); regional DTM 5 m (DXF, 2002-03) | 1 m / 5 m | 2008-2014 | CC BY 4.0 | web app | https://geoportale.regione.lazio.it/api/v2/resources?search=dtm&page_size=30 |
| Abruzzo | NATIONAL-OPEN | PST-A 1 m (48 %) | 1 m | 2008-2014 | CC BY 4.0 | web app | http://geoportale.regione.abruzzo.it/Cartanet/ |
| Molise | NATIONAL-OPEN | PST-A 1 m (86 %) | 1 m | 2008-2014 | CC BY 4.0 | web app | https://gn.mase.gov.it/portale/pst-dati-lidar |
| Campania | SUBNATIONAL-BETTER | CTR2020 DTM5K (photogrammetric); PST-A 1 m (71 %) | 5 m / 1 m | 2020 / 2008-13 | CC BY 4.0 | ArcGIS ImageServer, WCS | https://geomaps.regione.campania.it/server/rest/services/Amministrativo/CTR_DTM5m/ImageServer?f=json |
| Puglia | NATIONAL-OPEN | PST-A + MIADRA 1 m (57 %); regional DTM 8 m | 1 m / 8 m | 2007-2014 | CC BY 4.0 (PST-A) | web app; SIT behind login | https://gn.mase.gov.it/portale/distribuzione-dati-pst |
| Basilicata | NATIONAL-OPEN (35 % of grid); regional 5 m fills | PST-A 1 m; RSDI DTM/DSM 5 m | 1 m / 5 m | 2008-2010; 2013 | CC BY 4.0; IODL 2.0 | web app; WMS | https://gn.mase.gov.it/portale/distribuzione-dati-pst |
| Calabria | NATIONAL-OPEN | PST-A + MIADRA 1 m (90 %) | 1 m | 2008-2014 | CC BY 4.0 | web app | https://gn.mase.gov.it/portale/pst-dati-lidar |
| Sicilia | SUBNATIONAL-BETTER | SITR mdt_2013 (2 m, whole island); PST-A 1 m (48 %) | 2 m | 2012-2013 | CC BY 4.0 | ArcGIS ImageServer | https://map.sitr.regione.sicilia.it/gis/rest/services/modelli_digitali?f=json |
| Sardegna | SUBNATIONAL-BETTER | Regional 1 m lidar (coast 2008, towns 2013); 10 m full-island DTM | 1 m / 10 m | 2008-2013 | CC BY 4.0 (some CC BY-SA 3.0 IT) | app; WMS only | https://webgis.regione.sardegna.it/geoserverraster/ows?service=wms&version=1.3.0&request=GetCapabilities |
| San Marino | GLOBAL-ONLY | nothing published by the Republic found | n/a | n/a | n/a | n/a | https://www.gov.sm/pub1/GovSM/Dipartimenti/Dipartimento-Territorio-e-Ambiente/Ufficio-Tecnico-del-Catasto-e-Cartografia.html |
| Vatican City | NATIONAL-OPEN (by spillover) | Italian PST-A 1 m tiles over the east of the city | 1 m | flown 2 May 2008 | CC BY 4.0 | web app | http://wms.pcn.minambiente.it/ogc?map=/ms_ogc/wfs/QU_Lidar.map&service=WFS&version=1.1.0&request=GetFeature&typeName=EL.QUADRO.UNIONE.LIDAR.0101&PROPERTYNAME=regione,contratto,data,tavola&BBOX=41.88,12.43,41.92,12.47,urn:ogc:def:crs:EPSG::4326 |

### 2.6 Spain

| Jurisdiction | Verdict | Best source | Resolution | Year | Licence | Access | Source URL |
|---|---|---|---|---|---|---|---|
| Spain (national, IGN/CNIG) | NATIONAL-OPEN | PNOA-LiDAR third coverage + MDT50cm / MDS50cm; MDT02 (2nd cov), MDT05 (1st cov) | cloud at least 5 pts/m2; DTM 0.5 m (28,963 files so far), 2 m, 5 m | third coverage flown 2021-Mar 2026 | compatible with CC BY 4.0 (scne.es) | file-by-file (20 files without registration), WCS 5 m only | https://centrodedescargas.cnig.es/CentroDescargas/lidar-tercera-cobertura |
| Andalucía | NATIONAL-OPEN (regional 1 m marginal) | REDIAM MDE 1 m from the 2020-21 cloud | 1 m / 1.5 pts/m2 | 2020-21 | CC BY 4.0 | Nextcloud share | https://portalrediam.cica.es/geonetwork/srv/api/records/fb916b4f-76c4-4d8f-a3d2-4fd3641e509d |
| Aragón | NATIONAL-OPEN | PNOA re-distribution (IGEAR) | 0.5-1 pts/m2 clouds, 5 m DEM | 2010, 2016 | CC BY 4.0 | catalogue | https://icearagon.aragon.es/descargas.jsp |
| Asturias | NATIONAL-OPEN | PNOA (regional: contours, hillshade) | PNOA | 2020-21 / 2024-26 | CC BY (national) | CNIG | https://datos.gob.es/apidata/catalog/dataset/a03002951-elevaciones-asturias.json |
| Illes Balears | NATIONAL-OPEN | PNOA 3rd cov (2024); IDEIB MDT 2014 2 m | PNOA 5 pts/m2 | 2024; 2014 | CC BY-SA (IDEIB) | ArcGIS MapServer | https://ideib.caib.es/geoserveis/rest/services/public/GOIB_Altimetria_IB/MapServer/layers?f=pjson |
| Canarias | NATIONAL-OPEN | PNOA 3rd cov (Nov 2023-Jan 2024); open regional only 25 m | 5 pts/m2 | 2023-24 | CC BY 3.0/4.0 | CNIG; CKAN | https://opendata.sitcan.es/api/3/action/package_search?q=terreno&rows=20 |
| Cantabria | NATIONAL-OPEN | PNOA 3rd cov (2023); regional 1 m DSM, 5 m DTM images | 5 pts/m2 | 2023 | CC BY (national); regional unclear | CNIG | https://geoservicios.cantabria.es/inspire/rest/services/MDT/MapServer/layers?f=pjson |
| Castilla-La Mancha | NATIONAL-OPEN | PNOA | 5 pts/m2 | 2025-26 | CC BY 4.0 | CNIG | https://cartografia.castillalamancha.es/descargas-datos%20geograficos |
| Castilla y León | NATIONAL-OPEN | PNOA; regional MDT 5 m (2008-11) | 5 pts/m2 | 2025-26 | CC BY 4.0 | CNIG | https://idecyl.jcyl.es/geonetwork/srv/api/records/SPAGOBCYLCITDTSELMDT0811 |
| Catalunya | SUBNATIONAL-BETTER | ICGC Territorial LiDAR 3rd cov + DTM 0.5/1/5 m | at least 8 pts/m2, 6 cm RMSE | 2021-23; Sep 2026 update | CC BY 4.0 | open directory listings (single 422 GB file for 50 cm) | https://www.icgc.cat/en/Geoinformation-and-Maps/Data-and-products/Elevations/Territorial-elevations/Elevation-models |
| Comunitat Valenciana | NATIONAL-OPEN | PNOA 3rd cov (2023-24); ICV 1 m MDT from 2015-17 clouds | 5 pts/m2 | 2023-24 | CC BY 4.0 | sheet picker | https://dadesobertes.gva.es/api/3/action/package_search?q=MDT&rows=100 |
| Extremadura | NATIONAL-OPEN | PNOA | 5 pts/m2 | 2023 | CC BY 4.0 | CNIG | https://sitex.juntaex.es/SITEX/centrodescargas/view/5 |
| Galicia | NATIONAL-OPEN | PNOA 3rd cov (NPC01); IET classified 2015-16 cloud | 5 pts/m2 | 2024-26 | CC BY-compatible | viewer | https://ideg.xunta.gal/servizos/rest/services/Elevacions/Mdt2_PNOA09_10/ImageServer?f=pjson |
| La Rioja | NATIONAL-OPEN | PNOA 3rd cov (2024); regional 1 m DTM from 2016 | 5 pts/m2 | 2024 | CC BY 4.0 | WMS/WFS (hosts blocked) | https://iderioja.github.io/doc_lidar_2016_processing_en/dtm_dsm/digital_terrain_models/ |
| Madrid (Comunidad) | NATIONAL-OPEN | PNOA 3rd cov (2024); city MDT 10 cm grid | 5 pts/m2 | 2024 | CC BY 4.0 | CNIG | https://idem.comunidad.madrid/catalogocartografia/srv/spa/q?any=terreno&_content_type=json&fast=index&resultType=details&to=40 |
| Región de Murcia | NATIONAL-OPEN | PNOA | 5 pts/m2 | 2023-24 | CC BY 4.0 | CNIG | https://datosabiertos.regiondemurcia.es/api/3/action/package_search?q=lidar&rows=30 |
| Navarra | SUBNATIONAL-BETTER | Gobierno de Navarra MDT/MDS 50 cm + LAZ (2017, 2024) | 0.5 m; 5 pts/m2 (2024), 10-14 (2017) | 2017, 2024 | CC BY 4.0 | HTTP directories, WCS | https://filescartografia.navarra.es/6_MDE/_leeme.txt |
| País Vasco / Euskadi | SUBNATIONAL-BETTER (1 m, 2017); newest data national | GeoEuskadi MDT 1 m (WCS); Gipuzkoa LiDAR | 1 m, 2.2 pts/m2 | 2008, 2012, 2017 | free use with credit; CC BY 4.0 (Gipuzkoa) | WCS, bulk zips | https://www.geo.euskadi.eus/WCS_KARTOGRAFIA?request=getcapabilities&service=wcs |
| Ceuta | NATIONAL-OPEN | PNOA 3rd cov (Sep 2024); MDT/MDS 50 cm published | 5 pts/m2 | 2024 | CC BY 4.0 | CNIG | https://pnoa.ign.es/web/portal/pnoa-lidar/tercera-cobertura |
| Melilla | NATIONAL-OPEN | PNOA 3rd cov (Sep 2024); MDT/MDS 50 cm published | 5 pts/m2 | 2024 | CC BY 4.0 | CNIG | https://pnoa.ign.es/web/portal/pnoa-lidar/tercera-cobertura |

### 2.7 Portugal, Malta, Andorra, Monaco

| Jurisdiction | Verdict | Best source | Resolution | Year | Licence | Access | Source URL |
|---|---|---|---|---|---|---|---|
| Portugal (mainland) | NATIONAL-OPEN (file links need a free-account login) | DGT "LiDAR de Portugal continental" | MDT/MDS 0.5 m and 2 m; 10 pts/m2; ±10 cm | flown Apr 2024-Jul 2025; items to Jun 2026 | CC BY 4.0 | STAC (open); downloads via Keycloak login | https://cdd.dgterritorio.gov.pt/dgt-be/v1/collections |
| Azores | SUBNATIONAL-BETTER (same login) | Governo dos Açores "Cobertura Aérea LiDAR" | DTM/DSM 1/5/10 m, 15 pts/m2, RMSEz 9.3 cm | May-Oct 2024 | CC BY 4.0 | DGT CDD STAC + login | https://sma.idea.azores.gov.pt/geonetwork/srv/api/records/291673dc-2636-4d9f-9917-4434a5073588/formatters/xml |
| Madeira | SUBNATIONAL-BETTER | DROTe MDT do Arquipélago da Madeira (photogrammetric) | 2 m and 5 m, RMSEz 0.145 m | 2018-2019 data, files Jun 2026 | "uso sem condições" | direct ZIP, no login | https://snig.dgterritorio.gov.pt/rndg/srv/api/records/abba9a83-cb1c-4d2c-ad1d-ccb592247754/formatters/xml |
| Malta | NATIONAL-OPEN | Planning Authority DTM/DSM 2012 (1 m, WCS) | 1 m | 2012 | CC BY 4.0 | WCS 2.0.1 | https://malta.coverage.wetransform.eu/dtm_1m_2012/ows?SERVICE=WCS&VERSION=2.0.1&REQUEST=DescribeCoverage&COVERAGEID=dtm_1m_2012 |
| Andorra | RESTRICTED | Govern d'Andorra MDT | 15 m (50, 100 m) | about 2003 | exclusive IP of the Govern | download app with conditions | https://www.cartografia.ad/web-de-descarregues |
| Monaco | NATIONAL-OPEN (France's IGN LiDAR HD); own data GLOBAL-ONLY | IGN LiDAR HD, 20 published tiles over the principality | 0.5 m, 62 M points/km2 | flown to 27 Jun 2021, edition Jun 2025 | Licence Ouverte 2.0 | as France | https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=IGNF_LIDAR-HD_METADONNEE:metadata&resultType=hits&CQL_FILTER=BBOX%28geom%2C43.72%2C7.40%2C43.76%2C7.45%29 |

Other jurisdictions in the brief that appear inside the tables above: Liechtenstein (2.4), San Marino and Vatican City (2.5), Ceuta, Melilla, Canarias and Baleares (2.6), Corsica and the overseas departments (2.2).

## 3. Notes per jurisdiction

Agent labels in brackets say which sub-agent read the pages (uk, fr, de1, de2, chat, it1, it2, es1, es2, pt) or "lead" (me). All figures are as stated by the cited page unless marked "computed". The common licence for all OGL items is the Open Government Licence v3 (https://www.nationalarchives.gov.uk/doc/open-government-licence/version/3/), which allows copying, adapting and commercial and non-commercial exploitation with attribution.

### 3.1 United Kingdom, Crown Dependencies, Gibraltar, Ireland [uk]

**United Kingdom (GB-wide).** OS Terrain 50 is the only open GB-wide elevation product: 50 m grid, RMSE 4 m, "an Open Data product ... free to view, download and use", product version 2026-07, bulk downloads (ASCII grid 161.7 MB, GeoPackage 1.23 GB, MBTiles 1.07 GB) without a key: https://api.os.uk/downloads/v1/products/Terrain50 ; https://api.os.uk/downloads/v1/products/Terrain50/downloads ; https://docs.os.uk/os-downloads/products/land-and-terrain-portfolio/os-terrain-50/os-terrain-50-overview . OS Terrain 5 (5 m, RMSE 1.5 m urban / 2.5 m rural, 10,579 tiles) is "supplied as a download only from the OS Data Hub or OS Orders" and is not in the keyless open list; its Downloads URL returns 404; licence terms not read, treated as RESTRICTED: https://docs.os.uk/os-downloads/products/land-and-terrain-portfolio/os-terrain-5/os-terrain-5-overview ; https://docs.os.uk/os-downloads/products/land-and-terrain-portfolio/os-terrain-5/os-terrain-5-overview/product-supply.md ; https://api.os.uk/downloads/v1/products . APGB (Bluesky + Getmapping, DTM/DSM) is supplied to public-sector members only: https://marine.gov.scot/data-owners/apgb-aerial-photography-great-britain . Fallback: global products (the UK is not an EEA country; section 4.1).

**England.** Environment Agency National LIDAR Programme (302 survey blocks 2017 to Feb 2023, at least 1 pt/m2, 50 cm needs 4 pts/m2, 25 cm needs 16 pts/m2) and the LIDAR Composite DTM/DSM: 1 m "~99% of England", ±15 cm RMSE absolute, datum ODN, OGL. Composite built 2022 from surveys 2000-Apr 2022; the programme tile index (7,139 rows) has 565 rows with year 2023 or later and none from 2024, so nothing newer is published; recent record edits are metadata only. Access: 5 km tiles (GeoTIFF/LAZ) in the Defra Data Services Platform download app (URL not fetched) ; a live OGC WCS with elevation values for the 1 m DTM; OGC API Features tile index. The FAQ states no bulk country download is offered online. Sources: https://environment.data.gov.uk/dataset/9ba4d5ac-d596-445a-9056-dae3ddec0178 ; https://www.data.gov.uk/api/action/package_show?id=national-lidar-programme ; https://www.data.gov.uk/api/action/package_show?id=lidar-composite-digital-terrain-model-dtm-1m ; https://www.data.gov.uk/api/action/package_search?q=lidar+composite+dtm&rows=15 ; https://environment.data.gov.uk/support/faqs/275879146 ; https://environment.data.gov.uk/spatialdata/survey-index-files/ogc/features/v1/collections?f=json ; https://environment.data.gov.uk/spatialdata/lidar-composite-digital-terrain-model-dtm-1m/wcs?request=GetCapabilities&service=WCS&version=2.0.1 . NON-GOVERNMENTAL (public research body): UKCEH merged England+Wales 1 m DTM/DSM (OGL, 615 GB, published 2026-08-07), a repackaging of EA/NRW data, not finer: https://catalogue.ceh.ac.uk/documents/34443359-64c0-4909-9f27-bb5d47f1153f?format=json .

**Scotland.** The Scottish Land LiDAR Programme (Scottish Government, flown by Bluesky, May 2025 to Jul 2027) is the new national source: 10 pts/m2, 15 cm absolute vertical, first release 29 Jan 2026 (2.5 % of land), second release 12 Oct 2026 (almost 10 % of Scotland released, about a third flown), datum ODN; portal terms OGL v3 with 10 km tiles in a JavaScript app. Older: Phase I (2011-12, 1 m, about 2 pts/m2), Phases III-VI (4 pts/m2, 50 cm, 2015-22), Outer Hebrides 2019 (50 cm, some 25 cm), Orkney 2023 (16 pts/m2, 50 cm), Coastal Feb-Mar 2026 (at least 10 pts/m2, 50 cm, 792 km2). Phases I-VI "do not have full national coverage". Two Historic Environment Scotland sets (Scottish Ten 2010, 2016-17) carry a non-commercial licence. The JNCC OGC service is WMS only; WCS is disabled. Sources: https://blogs.gov.scot/digital/2026/01/29/scotlands-lidar-revolution-first-data-release-to-reveal-scotlands-landscape-in-unprecedented-detail/ ; https://blogs.gov.scot/digital/2026/10/12/scottish-lidar-user-conference-and-new-land-and-coastal-lidar-data-released/ ; https://remotesensingdata.gov.scot/about ; https://www.data.gov.uk/api/action/package_show?id=scottish-land-lidar-programme-dtm ; https://www.data.gov.uk/api/action/package_show?id=scottish-coastal-lidar-survey-2026-dtm ; https://srsp-ows.jncc.gov.uk/ows?service=wms&version=1.3.0&request=GetCapabilities . Contractors Bluesky and Fugro are commercial but publish nothing themselves.

**Wales.** Welsh Government LiDAR 2020-2023: 1 m DTM/DSM, 22,473 one-km tiles, flights 2020-2023 (45 tiles dated 2023, none later), published 27 Sep 2024, Wales-wide COGs (32-bit DTM 48,611,310,928 bytes; 16-bit DTM 3,601,904,014 bytes, by HEAD only), OGL on the download map (the tile-catalogue layer page says "Not Specified"). The NRW Historic LiDAR archive (25 cm to 2 m, about 70 % of Wales) is older. Sources: https://datamap.gov.wales/maps/6969 ; https://datamap.gov.wales/layers/geonode:welsh_government_lidar_tile_catalogue_2020_2023 ; https://datamap.gov.wales/layers/geonode:nrw_lidar_tile_catalogue_archive ; https://dmwproductionblob.blob.core.windows.net/cogs/lidar/wales_dtm_16bit_cog.tif (HEAD).

**Northern Ireland.** No NI-wide open lidar. OSNI Open Data 10 m DTM (293 sheets, "interpolated") and 50 m DTM; DfI River Basin LIDAR (1 m grids, 1 km tiles, 2004-2014, "not a supported LPS product"); Lough Neagh Environs 2017 (1 m); DAERA 2021 NI 3D Coastal Survey (whole coastline plus about 200 m inland; DTM, DSM, point cloud; Bluesky) and 2022 post-storm surveys. OGL. Sources: https://admin.opendatani.gov.uk/api/3/action/package_search?q=lidar&rows=30 ; https://admin.opendatani.gov.uk/api/3/action/package_show?id=osni-open-data-10m-dtm-sheets-1-50 ; https://tiles-eu1.arcgis.com/kswen6BYexuc1SUk/arcgis/rest/services/Topographic_LiDAR_DTM_TPK/MapServer?f=json .

**Jersey.** No open DEM/DTM; the Government of Jersey holds copyright of its digital mapping (5 m contours, 3D model; sold through Digimap (Jersey) Ltd). Jersey Heritage commissioned a lidar flight in 2020 for archaeology; no release found. opendata.gov.je CKAN returns no lidar/elevation/terrain datasets. https://www.gov.je/PlanningBuilding/JerseyMappingService/Pages/Digital.aspx ; https://jerseyeveningpost.com/news/2020/04/22/mystery-of-low-flying-plane-finally-solved/ ; https://opendata.gov.je/api/3/action/package_search?q=lidar&rows=10 .

**Guernsey (incl. Alderney, Sark, Herm).** Nothing official and open found. Bluesky flew a commissioned Channel Islands lidar DTM for a Guernsey bank for flood risk (commercial; date not read): https://theintermediary.co.uk/?p=16369 ; Digimap Ltd offers historic maps, no elevation: https://www.digimap.gg/ . No States of Guernsey GIS or open-data portal located.

**Isle of Man.** 2013 airborne lidar of the whole island (572 km2) for the Department of Infrastructure, DTM/DSM at 0.5, 1 and 2 m plus point cloud, resold by Bluesky Mapshop (2016 article): https://www.geoconnexion.com/news/bluesky-publishes-first-online-3d-maps-of-isle-of-man . Every gov.im page returned "Request Rejected" (WAF) or an empty body in this run, including a re-try and a Wayback attempt; not bypassed (section 5). A search result (not fetched, so not relied on) shows the gov.im "Business customers" mapping page listing 20 m and 50 m DTMs and LiDAR among its products.

**Gibraltar.** Nothing open found. Government drone-lidar of the Northern Defences (Dec 2023) is a heritage record only: https://www.gibraltar.gov.gi/press-releases/northern-defences-lidar-survey-and-current-works-8552023-9460 ; East View (NON-GOVERNMENTAL, commercial) says large-scale maps are contoured at 3 m to 60 m then 10 m: https://geospatial.com/country_profiles/gibraltar/ . A 2019 government release says its drone programme will yield orthophotos, 3D building models and contour maps to be provided through the geoportal (https://www.gibraltar.gov.gi/press-releases/department-of-the-environment-heritage-climate-change-invests-in-unmanned-aerial-vehicle-2422019-4808); geoportal.gov.gi answered 403, so whether any elevation grid is published there is unverified.

**Ireland.** Geological Survey Ireland "Open Topographic Lidar Data" (GSI, Discovery Programme/Heritage Council, TII, OPW, NYU Dublin, Westmeath surveys, 2015-2021): GeoTIFF DTM/DSM at 1 m (GSI), 0.13-0.14 m and 1 m (Discovery Programme), 2 m (TII, OPW), 0.25 m (Westmeath), datum Malin Head, CC BY 4.0 ("Contains Irish Public Sector Data (Geological Survey Ireland) licensed under ... CC BY 4.0"); coverage by project, no share stated. Download through a web viewer (not scripted); footprints on ArcGIS REST; the ImageServers listed there are hillshades (visualisation only). Tailte Éireann sells the nationwide height data (Series 1 10 m DTM ±3 m; Series 2 2 m / 10 m DTM, ±25 cm where lidar and ±1.5 m where photogrammetry, 2016-2019); the live page 404s, read via the Wayback copy of 20 Mar 2025; its open-data hub lists no DTM. Newer (not open): NPWS lidar+orthoimagery tender (EUR 4.6 m, closed 2026-07-17). Sources: https://data.gov.ie/api/3/action/package_search?q=lidar&rows=40 ; https://gsi.geodata.gov.ie/server/rest/services/Lidar?f=json ; https://gsi.geodata.gov.ie/imagehost/rest/services/Lidar?f=json ; https://web.archive.org/web/2026/https://tailte.ie/en/surveying/products/professional-mapping/height-data ; https://ie.openprocurements.com/tender/lidar-and-orthoimagery-services-for-national-parks-and-wildlife-service-npws-department-of-housing-l/ . Fallback: global products. Bathymetry note: INFOMAR (10 m inshore / 25 m / 100 m, CC BY 4.0) is in the same data.gov.ie answer.

### 3.2 France [fr]

**Metropolitan France, IGN LiDAR HD.** Specification at least 10 pulses/m2 (5 above 3200 m), classified COPC.LAZ cloud (11 classes, 1 km tiles of 0.5-2 GB), MNT/MNS/MNH 0.5 m GeoTIFF 1 km tiles, IGN69 heights, ZICAD no-fly zones are nodata. The WFS tile index (IGNF_LIDAR-HD_METADONNEE:metadata) holds 524,687 published tiles: 520,016 metropolitan (including Corsica) + 4,671 overseas; against 551,695 km2 for metropolitan France (a Wikipedia figure) this is about 94 % by tile count (an upper bound: edge tiles count whole). By end-of-flight year (metropolitan tiles): 2021 123,429; 2022 145,552; 2023 103,414; 2024 80,751; 2025 62,751; 2026 4,119 (latest flight end 2026-03-21); newest edition 2026-09-21. A NON-GOVERNMENTAL cross-check (TerraLab, from IGN data) gave 78 % of all-France communal area on 6 Sep 2026 (includes Guyane). Licence: Licence Ouverte 2.0 (free reproduction, adaptation, sharing, commercial or not, with attribution); the Geoplateforme applies a fair-use rate limit (HTTP 429). Access: tile download (cartes.gouv.fr), WMS-R GetMap with image/geotiff returning real elevation values (layers IGNF_LIDAR-HD_MNT/MNS/MNH_ELEVATION.ELEVATIONGRIDCOVERAGE.LAMB93, MaxWidth 5010, no fees), WMTS shaded layers, WFS tile index, no WCS. Vertical accuracy not read (the specification PDF geoservices.ign.fr/sites/default/files/2024-09/DC_LiDAR_HD_1-0.pdf now answers with the cartes.gouv.fr web page instead of a PDF, and an earlier copy had no extractable text). RGE ALTI 1 m and 5 m (mixed sources; updates stopped in 2024) and BD ALTI 25/75 m are the fallback for the unpublished 6 %. Sources: https://www.data.gouv.fr/api/1/datasets/675871bd6e60265b46ab356e/ ; https://data.geopf.fr/wms-r?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetCapabilities ; https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=IGNF_LIDAR-HD_METADONNEE:metadata&resultType=hits ; https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=DescribeFeatureType&TYPENAME=IGNF_LIDAR-HD_METADONNEE:metadata ; https://cartes.gouv.fr/cgu ; https://fr.wikipedia.org/wiki/France_m%C3%A9tropolitaine ; https://www.data.gouv.fr/api/1/datasets/61489f5ae64c7f138f1b2e15/ ; https://www.data.gouv.fr/api/1/datasets/?q=BD+ALTI&page_size=8 ; https://www.data.gouv.fr/api/1/datasets/?q=lidar&page_size=50&page=2 .

**France, regional, departmental and city lidar.** Beats IGN locally: Eurométropole de Strasbourg (flown 17-21 Mar 2025, at least 100 pts/m2 emitted, 3 cm XY / 6 cm Z, COPC.LAZ + MNT 50 cm COG, Licence Ouverte 2.0; a 2021 edition also exists): https://data.strasbourg.eu/api/explore/v2.1/catalog/datasets/nuage_de_points_3d_2025 ; https://data.strasbourg.eu/api/explore/v2.1/catalog/datasets/raster_elevation_sol_2025 ; https://www.sig.strasbourg.eu/datastrasbourg/3d_fiches_metadonnees_pdf/2025/metadata_nuage_de_points.pdf . Métropole de Lyon LiDAR 2023: 30 pts/m2 average (up to 100 in dense urban), Z 10 cm, 657 km2, classed LAZ + MNT 50 cm COG, Licence Ouverte: https://www.data.gouv.fr/api/1/datasets/67f59d2966c82acad240a1cb/ ; https://www.data.gouv.fr/api/1/datasets/6836523318d64a73d170837a/ . Bordeaux Métropole lidar May 2020 (15 pts/m2, 620 km2, Licence Ouverte v1); its 2023 50 cm MNT is mostly photogrammetric and not better than IGN; CRAIG (Auvergne-Rhône-Alpes) LiDARAURA 2021 (14 pts/m2 over 21 zones, 3,582 km2) and the Lille MEL 2018/2020 lidar: https://www.data.gouv.fr/api/1/datasets/?q=lidar&page_size=50 . Rennes Métropole MNT 2021 50 cm (ODbL), not clearly better than IGN: https://www.data.gouv.fr/api/1/datasets/?q=lidar&page_size=50 . Regions: Bretagne (Morbihan coast 2010 only), Pays de la Loire (one local MNT), Nouvelle-Aquitaine (yearly coastal lidar 1 m, Litto3D), Occitanie (coastal studies), Auvergne-Rhône-Alpes (CRAIG), Grand Est (Strasbourg, Nancy, Ill 2009), Hauts-de-France (MNT HdF 1 m is RGE ALTI) have nothing regional-wide that beats IGN; PACA/Sud, Normandie, Centre-Val de Loire, Bourgogne-Franche-Comté and Île-de-France: no lidar dataset found (some catalogues blocked, section 5).

**Corsica.** IGN LiDAR HD, 9,321 tiles in the Corsica bounding box (my recount with the lat,lon axis order: 9321; fr agent's count the same), missions 21LHDCorseUS/UT and 22LHDZICorse_v2, Riegl VQ-780 II-S, median about 42 M points per km2 (sample of 300 tiles), flights 2021-05-04 to 2022-09-10, edition 2025-04-02. No Collectivité de Corse dataset (data.gouv searches return 0). Source: https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=IGNF_LIDAR-HD_METADONNEE:metadata&resultType=hits&CQL_FILTER=BBOX%28geom%2C41.3%2C8.4%2C43.1%2C9.7%29 .

**Overseas departments (named only).** IGN LiDAR HD tiles so far for the Guadeloupe area (2,006) and La Réunion (2,665), none found for Martinique, Mayotte or Guyane; RGE ALTI / BD ALTI cover the DROM; the Collectivité de Guyane publishes local 1 m MNT/MNS (Mana 2023). https://www.data.gouv.fr/api/1/datasets/675871bd6e60265b46ab356e/ .

### 3.3 Belgium, Netherlands, Luxembourg [fr]

**Flanders.** DHMV II: DTM and DSM 1 m (also 5 m; hillshade 25 cm), point cloud 8 pts/m2 per strip (about 16 average), RMSE z 0.05 m, flown 2013-2015; heights Ostend (EPSG:5710). DHMV III flights: season 1 1 Jan-15 Apr 2026, season 2 1 Nov 2026-15 Apr 2027, products expected 2028. Rasters under the Flemish model licence for free reuse; the raw point cloud's terms are unclear (the metadata restricts to public-interest tasks after registration, the Geopunt page says everyone). WCS free; Brussels is inside its extent. Sources: https://www.vlaanderen.be/digitaal-vlaanderen/onze-diensten-en-platformen/earth-observation-data-science-eodas/het-digitaal-hoogtemodel/digitaal-hoogtemodel-vlaanderen-ii ; https://www.vlaanderen.be/digitaal-vlaanderen/nieuws/dit-jaar-start-de-vernieuwing-van-het-digitaal-hoogtemodel ; https://metadata.vlaanderen.be/srv/api/records/f52b1a13-86bc-4b64-8256-88cc0d1a8735 ; https://www.vlaanderen.be/digitaal-vlaanderen/onze-oplossingen/open-data/voorwaarden-voor-het-hergebruik-van-overheidsinformatie ; https://metadata.vlaanderen.be/srv/api/records/242ddabc-3cbc-44f0-9623-bee874b29549 ; https://www.vlaanderen.be/geopunt/kaarttoepassingen/eodas-open-lidar-portaal .

**Wallonia.** SPW "Relief de la Wallonie" LiDAR 2021-2022: planned 6.8 pts/m2, MNT/MNS/MNH 0.5 m and 1 m, altimetric RMSE at most 0.4 m (spec), Lambert 2008 (EPSG:3812), CC BY 4.0, direct ZIPs per province, ATOM, ArcGIS REST, LAZ per province. Sources: https://metawal.wallonie.be/geonetwork/srv/api/records/3ef17388-54cd-4034-baad-d247243123c2 ; https://metawal.wallonie.be/geonetwork/srv/api/records/ab14b035-c9b0-4c79-a2b6-36811fca96a1 ; https://metawal.wallonie.be/geonetwork/srv/api/records/a004e570-99d6-4fe5-b83d-49b774409278 .

**Brussels.** UrbIS lidar flown 9 Oct 2021, Riegl VQ-1560 II, 67 pts/m2 average; DTM raster 0.5 m (CC0 1.0), DSM, contour lines, point cloud (CC BY 4.0), Atom feed. https://geobru-geonetwork.irisnet.be/geonetwork/srv/api/records/1d7bd49d-fe83-4388-af85-6f5dc8ec7909 ; https://geobru-geonetwork.irisnet.be/geonetwork/srv/api/records/ff1124e1-424e-11ee-b156-00090ffe0001 ; https://urbisdownload.datastore.brussels/atomfeed/1d7bd49d-fe83-4388-af85-6f5dc8ec7909-en.xml .

**Belgium (national, NGI).** NGI sells "Digitaal terreinmodel niveau 1" (DTED, 1:100,000 scale), EUR 616.02 per 1°x1° or EUR 6,771.46 for all Belgium, copyright NGI; no national open lidar DTM; the NGI open-data page returns 404. https://metadata.vlaanderen.be/srv/api/records/82271D4B-EBDF-4289-BBB1-69C1C96223DB .

**Netherlands.** AHN: DTM and DSM 0.5 m and 5 m GeoTIFF plus LAZ clouds; AHN4 about 10-14 pts/m2; stated vertical accuracy for AHN2-4 at most 5 cm standard deviation and at most 5 cm systematic. Status Nov 2026: AHN5 online in three parts (west parts on 2 Oct 2024); first part of AHN6 (north-east, flown spring 2025, cloud coloured from aerial photos) online 13 Oct 2025, CC BY 4.0; the 2026 flight-line map shows AHN6 lines being flown, so 2026/27 data is not yet published. AHN4 DTM on data.overheid.nl is CC0, "minimaal 10" pts/m2. PDOK ATOM serves COG sheets; AHN5/AHN6 WMS/WCS go via Ellipsis Drive. Caribbean Netherlands (Bonaire, Sint Eustatius, Saba): lidar 2023-24, viewers live since 19 Dec 2024. Not read: AHN5 acquisition years and density, AHN5/6 accuracy. Sources: https://www.ahn.nl/ ; https://www.ahn.nl/eerste-deel-ahn-6-beschikbaar ; https://www.ahn.nl/dataroom ; https://www.ahn.nl/uitgelicht ; https://www.ahn.nl/waar-vliegen-we-nu ; https://www.ahn.nl/kwaliteitsbeschrijving ; https://service.pdok.nl/rws/ahn/atom/dtm_05m.xml ; https://data.overheid.nl/en/dataset/47567-actueel-hoogtebestand-nederland--ahn--dtm .

**Luxembourg.** BD-L-Lidar2024 (Administration du cadastre et de la topographie): over 30 pts/m2 emitted, classified COPC.LAZ (10 classes) in 500 m tiles, MNT and MNS 50 cm COG as country-wide single files (MNT 40,131,787,860 bytes by HEAD), flown 2024-02-28 to 2024-04-30, CC0, LUREF-LTM; the 2019 survey (15 pts/m2, ±6 cm vertical) precedes it. https://data.public.lu/api/1/datasets/?q=lidar&page_size=20 ; https://data.public.lu/api/1/datasets/lidar-2024-releve-3d-du-territoire-luxembourgeois/ ; https://download.data.public.lu/resources/bd-l-lidar2024-releve-3d-du-territoire-luxembourgeois/20241223-093912/MNT_Lidar2024.tif (HEAD).

### 3.4 Germany [de1: federal and BW, BY, BE, BB, HB, HH, HE, MV; de2: NI, NW, RP, SL, SN, ST, SH, TH]

**Federal (BKG).** The only free federal elevation products are DGM200 (200 m GeoTIFF, 36 MB, dl-de/by-2-0 "geldleistungsfrei", re-derived 06.11.2026) and DGM1000, plus an INSPIRE DGM200 WCS/WMS. The nationwide DGM1 (1 m GeoTIFF float32, 1 km tiles, EPSG:25832, about 1 TB, a mosaic of the 16 Land datasets whose age varies from 1 to over 15 years per BKG's age table) is licensed against a fee through ZSGT/AdV "ab 8.000,00 EUR" with a licence agreement; DOM1 (Stand 2021) only as a nationwide data-carrier delivery; DGM5 only for federal authorities and V GeoBund users (no public supply since 08.06.2024); DGM25 for V GeoBund users. DGM10/DGM50 are not in the current catalogue. A federal open-data list (106 items) contains only DGM1000, DGM200 and the INSPIRE DGM200 services as elevation. Not allowed for a personal offline map: DGM1/DOM1/DGM5/DGM25 (paid or restricted); allowed: DGM200/DGM1000. Regional federal product: DGM-W Elbe 2022 (BfG; 4,043 km2, 1 m GeoTIFF incl. river bed, DHHN2016, CC BY 4.0, Zenodo 7.3 GB zip, announced 21.11.2025). Sources: https://gdz.bkg.bund.de/index.php/default/digitale-geodaten/digitale-gelandemodelle/digitales-gelandemodell-gitterweite-1-m-dgm1.html ; https://gdz.bkg.bund.de/index.php/default/digitale-geodaten/digitale-gelandemodelle.html ; https://gdz.bkg.bund.de/index.php/default/digitale-geodaten/digitale-gelandemodelle/digitales-gelandemodell-gitterweite-200-m-dgm200.html ; https://gdz.bkg.bund.de/index.php/default/digitale-geodaten/digitale-gelandemodelle/digitales-gelandemodell-gitterweite-5-m-dgm5.html ; https://gdz.bkg.bund.de/index.php/default/digitale-geodaten/digitale-gelandemodelle/digitales-gelandemodell-gitterweite-25-m-dgm25.html ; https://gdz.bkg.bund.de/index.php/default/digitale-geodaten/digitale-gelandemodelle/digitales-oberfaechenmodell-dom1.html ; https://gdz.bkg.bund.de/index.php/default/open-data.html ; https://sg.geodatenzentrum.de/web_public/gdz/aktualitaet/dgm1.html ; https://www.govdata.de/dl-de/by-2-0 ; https://www.bafg.de/DE/5_Informiert/4_Infothek/Aktuelles/_doc/2025/251121_DGM-W-Elbe.html ; https://zenodo.org/api/records/17378779 . NON-GOVERNMENTAL cross-check used by de1 for all Länder: the GitHub compilation https://raw.githubusercontent.com/wiesehahn/lidar-availability/main/availability_germany.md .

**Baden-Württemberg (LGL).** DGM1 and DGM025 (0.25 m GeoTIFF) are "in der Regel Open Data" (dl-de/by-2-0, attribution "Datenquelle: LGL, www.lgl-bw.de, dl-de/by-2-0"); DOM1/DOM5/nDOM also open. ALS_2 (2016-2021, statewide) and ALS_3 (since 2022, expected finished about 2029, at least 8 pts/m2 from 2023 in flown areas) point clouds carry the open licence but are ordered by e-mail for a fee (minimum EUR 60 + VAT). Datum DHHN2016. Access: Open GeoData portal (Angular app, ZIP) and an INSPIRE WCS 2.0.1; the WCS metadata still describes the 2000-2005 flight. https://www.lgl-bw.de/Produkte/3D-Produkte/Digitale-Gelaendemodelle/index.html ; https://www.lgl-bw.de/Produkte/3D-Produkte/Laserscandaten/ALS_2/ ; https://www.lgl-bw.de/Produkte/3D-Produkte/Laserscandaten/ALS_3/ ; https://www.lgl-bw.de/Open-GeoData ; https://owsproxy.lgl-bw.de/owsproxy/wcs/WCS_INSP_BW_Hoehe_Coverage_DGM1?SERVICE=WCS&REQUEST=GetCapabilities&VERSION=2.0.1 ; https://metadaten.geoportal-bw.de/geonetwork/srv/api/records/45f2f73c-a258-442d-9df4-ab688c5efe40 .

**Bayern (LDBV).** The strongest Land: OpenData Bayern DGM1 (1 m, statewide, DHHN2016, better than ±0.2 m), DGM5 (XYZ), open LAZ point cloud (at least 4 pts/m2 for flights since 2012), DOM20 (20 cm surface). CC BY 4.0, "Eine Anfrage ... nicht erforderlich", no registration. Flight blocks 2016-2025 computed from the flight KML: 2016 2,074; 2017 2,042; 2018 9,627; 2019 7,694; 2020 7,322; 2021 3,777; 2022 8,964; 2023 7,935; 2024 8,652; 2025 13,810 km2 (sum about 71,900 vs 70,549 km2: reflown over ten winters); the 2025 blocks have flight dates 06.03.2025 to 12.04.2026 and whether they are already in the downloads is not stated. Access: single 1 km tiles (~4 MB), Metalink per municipality / district, whole-state DGM1 about 240 GB (22 MB metalink), DGM5 XYZ about 14 GB, LAZ up to 1 GB per tile, REST height service. https://www.geodaten.bayern.de/odd/m/3/html/nutzungsbedingungen.html ; https://geodaten.bayern.de/opengeodata/json/opengeodata_produkte.json ; https://www.geodaten.bayern.de/odd/m/3/html/datenhinweise/datenhinweise_dgm.html ; https://www.ldbv.bayern.de/produkte/landschaftsinformationen/gelaende.html ; https://www.ldbv.bayern.de/produkte/landschaftsinformationen/laser.html ; https://www.geodaten.bayern.de/download/uebersicht_DGM/Laserscanningbefliegungen.kml ; https://geodaten.bayern.de/odd/a/dgm/dgm1/meta/metalink/09.meta4 (HEAD).

**Berlin.** ATKIS DGM1 (1 m xyz, 2x2 km tiles, ATOM) updated by stereo-photogrammetry from imagery (03/2022, 04/2023, 03/2025, 08/2025; BKG's table shows all 891 km2 at age 1); ALS point cloud (LAS 1.4 classified, 9.8 pts/m2, 1x1 km, flights 24.02., 25.02., 02.03.2021); DOM1 from ALS. dl-de/zero-2-0 with "keine Zugriffsbeschränkungen" (no attribution needed). Accuracy ±10 cm + 5 % of grid (flat open), 2 sigma; DHHN2016. https://gdi.berlin.de/geonetwork/srv/api/records/f0e8ff09-2887-3446-9c53-81dbc45af03c ; https://fbinter.stadt-berlin.de/fb_daten/beschreibung/dgm1.html ; https://gdi.berlin.de/data/dgm1/atom/ ; https://gdi.berlin.de/data/a_als/atom/ ; https://gdi.berlin.de/geonetwork/srv/api/records/85a97801-36bb-4627-8790-f5e53b1e38b3 ; https://www.berlin.de/sen/stadt/stadtdaten/geoinformation/landesvermessung/geotopographie-atkis/dom-digitales-oberflaechenmodell/ .

**Brandenburg (LGB).** DGM 1 m statewide (100 % per metadata, updated 27.04.2025, published 29.05.2026, three-year cycle; WCS coverage bb_dgm), ALS point cloud (LAZ 1.4, about 5 pts/m2, 1 km tiles, published 16.09.2025; GeoBroker says 60 %, my count of 13,086 tiles of 1 km2 against 29,654 km2 gives about 44 %, an estimate; 1 pt/m2 for 2008-2012 patches). "Alle digitalen Geobasisdaten des Landes Brandenburg sind Open Data", dl-de/by-2-0. Open directories, WCS/WMS/WMTS. https://geobasis-bb.de/lgb/de/geodaten/open-data/ ; https://geobasis-bb.de/lgb/de/geodaten/3d-produkte/gelaendemodell/ ; https://geobasis-bb.de/lgb/de/geodaten/3d-produkte/laserscandaten/ ; https://geobroker.geobasis-bb.de/gbss.php?MODE=GetProductInformation&PRODUCTID=518094b9-d294-4e99-aeb2-550592a00682 ; https://geobroker.geobasis-bb.de/gbss.php?MODE=GetProductInformation&PRODUCTID=d9895ec2-7039-4c0d-914c-a68f227a7069 ; https://isk.geobasis-bb.de/ows/dgm_wcs?SERVICE=WCS&REQUEST=GetCapabilities&VERSION=2.0.1 ; https://data.geobasis-bb.de/geobasis/daten/dgm/ ; https://data.geobasis-bb.de/geobasis/daten/als/laz/ .

**Bremen.** ATKIS DGM1 and DGM5, DOM1/DOM5 open since 9 June 2024 for Stadt Bremen and Bremerhaven (CC BY 4.0, read in the Wayback copy of the MetaVer record because metaver.de answered 429); DGM1 basis ALS Bremerhaven 2015, Bremen 2017; a new ALS flight on 18-19 March 2026 is not yet published; raw laser data "erhältlich" on request (a GitHub compilation lists EUR 80/km2, NON-GOVERNMENTAL, not on an official page). https://www.geo.bremen.de/produkte/3d-produkte/hoehenmodelle-12482 ; https://www.geo.bremen.de/produkte/open-data-produktuebersicht-15654 ; https://web.archive.org/web/2026/https://metaver.de/trefferanzeige?docuuid=C448202A-0F15-413A-99E6-F22F10E190C9 .

**Hamburg.** "Geländemodelle Hamburg - DGM": 1 m yearly ZIPs (ASCII to 2021, GeoTIFF 2022; the 2022 ZIP is 1,364,773,543 bytes), flights 2010, 2020, 2022, single-point accuracy about ±15 cm on roads; dl-de/by-2-0; a new ALS flight Jan-Mar 2026 is unpublished. No open point cloud (CKAN searches for Laserscan/Punktwolke find none; only the GitHub compilation says it is withheld for privacy). https://suche.transparenz.hamburg.de/api/3/action/package_show?id=gelaendemodelle-hamburg-dgm2 ; https://archiv.transparenz.hamburg.de/hmbtgarchive/HMDK/dgm1_hh_2022-04-30_258334_snap_1.zip (HEAD) ; https://suche.transparenz.hamburg.de/api/3/action/package_search?q=Punktwolke&rows=40&fl=name,title,metadata_modified ; https://www.hamburg.de/politik-und-verwaltung/behoerden/behoerde-fuer-stadtentwicklung-und-wohnen/aemter-und-landesbetrieb/landesbetrieb-geoinformation-und-vermessung/wir-ueber-uns/laserscanning-und-vermessungsbefliegungen-1136924 .

**Hessen (HVBG).** ATKIS DGM1 and DOM1 (1 m GeoTIFF float32) free in the gds.hessen.de Downloadcenter; open data since 01.02.2022 ("Jede Nutzung ... ohne Einschränkung oder Bedingung erlaubt", no named licence). ALS point cloud (LAS 1.3/LAZ, at least 4 pts/m2, ±15 cm / ±30 cm) with fees by time spent; the state has been fully scanned since 2014, Laserscan 2 closed 2021, Laserscan 3 is planned in winter blocks 2021/22 to 2026/27 (Stand 15.01.2022; which blocks are done is not verified); the GitHub compilation says the cloud is open, conflicting with HVBG's fee wording. https://hvbg.hessen.de/geoinformation/open-data ; https://hvbg.hessen.de/landesvermessung/geotopographie/3d-daten/digitale-gelaendemodelle ; https://hvbg.hessen.de/landesvermessung/geotopographie/3d-daten/airborne-laserscanning ; https://hvbg.hessen.de/sites/hvbg.hessen.de/files/2024-06/laserscan_3._befliegungsuebersicht-bf.pdf .

**Mecklenburg-Vorpommern (LAiV).** DGM1 (also DGM5/DGM25, 2x2 km tiles, XYZ zip and GeoTIFF; CC BY 4.0, ATOM feed updated 2025-02-01, about 10-year update cycle), DOM 1 m, bDOM 0.10 m. ALS point cloud (LAZ, at least 2 pts/m2) is "entgeltpflichtig" (paid). https://www.geodaten-mv.de/dienste/dgm_atom ; https://www.laiv-mv.de/static/LAIV/Geoinformation/Dateien/Geobasisdaten/Kurzbeschreibung_ATOM_DGM.pdf ; https://laiv.geodaten-mv.de/afgvk/Geotopographie/Download?produkt=DGM1 ; https://www.laiv-mv.de/Geoinformation/Geobasisdaten/Oberflaechenmodelle/ .

**Niedersachsen (LGLN).** Open DGM1/DOM1 COGs (1 m, 1 km tiles, from ALS at least 4 pts/m2, CC BY 4.0; STAC, WCS, WMS; open since 2024-06-09). The open DGM1 is old: LGLN's map (Stand 22.10.2026) shows 75.4 % from flights more than 9 years old, 1.0 % from 2022, 11.8 % 2021, 6.9 % 2020, 1.8 % 2019, 3.1 % 2018, nothing 2023-26; the ordered point cloud (3D-Messdaten, LAZ, last/only return, ordered, KOVerm fees) has far newer flights (map Stand 12.11.2026: 2025 15.2 %, 2023 1.4 %, 2022 2.9 %, 2021 11.8 %). The STAC "datetime" (newest 2025-03-09) is unreliable for flight year. https://dgm.stac.lgln.niedersachsen.de/collections ; https://dgm.stac.lgln.niedersachsen.de/collections/dgm1/items?limit=1&sortby=-datetime ; https://www.lgln.niedersachsen.de/startseite/wir_uber_uns_amp_organisation/allgemeine_geschafts_und_nutzungsbedingungen_agnb/allgemeine-geschafts-und-nutzungsbedingungen-agnb-97401.html ; https://lgln-geodaten.niedersachsen.de/startseite/luftbilder_und_3d_produkte/3d_produkte/3d_messdaten/3d-messdaten-142870.html ; https://cms.lgln.niedersachsen.de/fb24/dgm1_dom1.pdf ; https://cms.lgln.niedersachsen.de/fb24/aktualitaet_als.pdf ; https://ni-lgln-opengeodata.hub.arcgis.com/api/search/v1/collections/all/items?q=Punktwolke&limit=20 .

**Nordrhein-Westfalen (Geobasis NRW).** Open 3D-Messdaten (LAZ, 35,860 tiles, 3,487.8 GB, 4.00 pts/m2 declared, ±15 cm / ±30 cm) and DGM1 (GeoTIFF, 78.8 GB) plus DOM1; DGM1 tile years 2020-2025 (2020:1,532; 2021:7,142; 2022:8,770; 2023:5,004; 2024:7,772; 2025:5,640), five-year cycle; the laser survey was suspended for winter 2025/26 and resumes in 2026/27 with at least 8 pts/m2. dl-de/zero-2-0 ("ohne Einschränkungen oder Bedingungen"). Bulk XML indexes, no login; WCS. https://www.opengeodata.nrw.de/produkte/geobasis/hm/ ; https://www.opengeodata.nrw.de/produkte/geobasis/hm/3dm_l_las/3dm_l_las/ ; https://www.opengeodata.nrw.de/produkte/geobasis/hm/dgm1_tiff/dgm1_tiff/ ; https://www.opengeodata.nrw.de/produkte/geobasis/hm/3dm_l_las/3dm_l_las/3dm_meta.zip ; https://www.bezreg-koeln.nrw.de/geobasis-nrw/produkte-und-dienste/hoehenmodelle/3d-messdaten ; https://www.bezreg-koeln.nrw.de/geobasis-nrw/produkte-und-dienste/hoehenmodelle/digitale-gelaendemodelle/digitales-gelaendemodell ; https://www.govdata.de/dl-de/zero-2-0 .

**Rheinland-Pfalz (LVermGeoRP).** Open data (dl-de/by-2-0, "keine Zugriffsbeschränkungen"): DGM1/DGM5/DGM10, DOM1, bDOM and the laser points LPG/LPO (about 4 pts/m2; height 0.15 m open terrain); Aktualität DGM1 15.03.2022-15.03.2026, LPG 15.03.2022-01.04.2025, a quarter of the state renewed per year since 2022. Free through the geoshop (the cart flow was not run, so "no login" is not verified). https://lvermgeo.rlp.de/geodaten-geoshop/open-data ; https://lvermgeo.rlp.de/produktinformationen/geotopografie/3d-geodaten/laserpunkte-gelaende-lpg-laserpunkte-objekte-lpo ; https://lvermgeo.rlp.de/produktinformationen/geotopografie/3d-geodaten/digitale-gelaendemodelle-dgm ; https://metaportal.rlp.de/gui/html/7c359d75-c876-4078-b489-fcfd3332a8bd ; https://metaportal.rlp.de/gui/html/ab69aa3d-e786-41f8-95dc-7b34abb06c41 ; https://geoshop.rlp.de/opendata-dgm1.html ; https://lvermgeo.rlp.de/geodaten-geoshop/open-data/hinweise-zum-download .

**Saarland (LVGL).** "DGM1 (2025)": 1 m GeoTIFF tiles on a public LVGL Nextcloud share and via ATOM/WMS, dl-de/by-2-0, metadata dated 2025-11-05 ("Laserscanbefliegung 2025"). No open point cloud found; official LVGL pages are behind a bot-shield (403, not bypassed), so accuracy, density and flight dates are not verified. https://geoportal.saarland.de/app-article/geobasisdatenuebersicht/ ; https://www.shop.lvgl.saarland.de/index.php?option=com_content&view=article&id=18&metaauthor=odata ; https://geoportal.saarland.de/mapbender/php/mod_dataISOMetadata.php?outputFormat=iso19139&id=d73285be-e3a1-46ef-9562-88883dc0f9c0 .

**Sachsen (GeoSN).** DGM1 and DOM1 (GeoTIFF, 2x2 km tile ZIPs, EPSG:25833, DHHN2016), Laserscandaten (LAZ, ground 2, non-ground 20; ±0.15 m / ±0.30 m, 95 %); flights 2018-2025 by area (map of 26.01.2026; updates planned winter 2025/26 to 2030/31); dl-de/by-2-0, free download; point density not stated in what was read. https://www.geodaten.sachsen.de/downloadbereich-digitale-hoehenmodelle-4851.html ; https://www.landesvermessung.sachsen.de/download-dienste-und-testdaten-8647.html ; https://www.landesvermessung.sachsen.de/fachliche-details-8645.html ; https://www.landesvermessung.sachsen.de/prod_dhm_stadt/dhm_stadt.pdf ; https://www.landesvermessung.sachsen.de/planung-aktualitaet-8643.html ; https://www.landesvermessung.sachsen.de/allgemeine-nutzungsbedingungen-8954.html ; https://www.geodaten.sachsen.de/batch-download-4719.html .

**Sachsen-Anhalt (LVermGeo).** Open DGM1 (1 m; also DGM5 ZIP 1.62 GB), DOM1 and bDOM20, dl-de/by-2-0; DGM1 tile years 2021-2025 (Stand 05.12.2025), laser-flight map 2015-2026 (2026 already flown near Magdeburg, Harz and Halle). The 3D-Messdaten point cloud (4-8 pts/m2) is on request with a fee table (the meaning of an EUR 190 "Open Data" row is ambiguous). https://www.lvermgeo.sachsen-anhalt.de/de/gdp-open-data.html ; https://www.lvermgeo.sachsen-anhalt.de/de/gdp-dgm-dom-lsa.html ; https://www.lvermgeo.sachsen-anhalt.de/de/gdp-3d-messdaten.html ; https://www.lvermgeo.sachsen-anhalt.de/de/datei/download/id/17600,501/dgm1_st_aktualitaetsuebersicht_2025_12_05.pdf ; https://www.lvermgeo.sachsen-anhalt.de/de/datei/download/id/416049,501/laserscan_befliegungen_st.pdf .

**Schleswig-Holstein (LVermGeo SH).** DGM1 (XYZ ASCII, 1 km units, from ALS, 3-4 pts/m2, ±30 cm position / ±15 cm height) and bDOM 20 cm (image matching, not laser) via the OpenGBD download client; CKAN says CC BY 4.0 (issued 2025-03-07, modified 2026-10-26); one metadata description says "Laserscan (DGM1) von 2007", so flight years and staleness are unverified; no open ALS cloud found. The live download host fails TLS verification (read via Wayback). https://www.schleswig-holstein.de/DE/Landesregierung/LVERMGEOSH/Service/serviceGeobasisdaten/geodatenService_Geobasisdaten_DGM.html ; https://opendata.schleswig-holstein.de/api/3/action/package_search?q=DGM1&rows=10 ; https://web.archive.org/web/2026/https://geodaten.schleswig-holstein.de/gaialight-sh/_apps/dladownload/dl-dgm1.html .

**Thüringen (TLBG).** Höhendaten 2020-2025: DGM1/DOM1 (GeoTIFF + XYZ, 1 m, 1 km tiles) and Laserscandaten LAZ (LAS 1.4, 4 pts/m2, ±30 cm / ±15 cm; DHHN2016), download periods 1996-2006, 2010-2013, 2014-2019, 2020-2025; the LAS feed updated 2026-05-11. Licence stated twice (dl-de/by-2-0 in the ATOM feed, CC BY 4.0 in the CSW record). The live geoportal showed an outage page and CAPTCHAs, so everything was read from Wayback copies and the CSW; coverage share of 2020-2025 not measured. https://web.archive.org/web/2026/https://geoportal.geoportal-th.de/gaialight-th/_apps/dladownload/dl-dhm.html ; https://web.archive.org/web/2026/https://geoportal.geoportal-th.de/hoehendaten/Uebersichten/README_Hoehendaten_2020-2025.pdf ; https://web.archive.org/web/2026/https://geoportal.geoportal-th.de/dienste/atom_th_hoehendaten_las ; https://tlbg-onlineshop.thueringen.de/onlineshop/uebersichten/digitale-gelaendemodelle-dgm .

**Germany coast/federal waters.** Not researched (no BSH/WSV pages fetched).

### 3.5 Switzerland, Liechtenstein, Austria [chat]

**Switzerland (swisstopo).** swissALTI3D (DTM): 0.5 m and 2 m COG GeoTIFF, 1 km tiles (about 43,500), oversampled to 0.5 m where density is low; source classes (1 sigma, 3D): new lidar 0.3 m, previous lidar (below 2000 m) 0.5 m, stereo correlation (above 2000 m) 1-3 m, manual edits 0.1-1 m; datum LV95 / LN02; six-year update cycle, open since 2021, STAC collection updated 2026-09-26 with a tile dated 2026; full 0.5 m set 770 GB, 2 m 44 GB. swissSURFACE3D classified point cloud: from 2024 at least 15 pts/m2 (mean 25-40, COPC/LAZ 1.4, about 200 MB per tile); until 2024 at least 5 (mean 15-20); accuracy ±20 cm planimetric, ±10 cm altimetric; the new campaign runs in six stages 2024-2029, data online about 12 months after flight, stage 1 (St. Gallen, AR, AI, Glarus, Liechtenstein) published 2026-02-17. swissSURFACE3D Raster (DSM 0.5 m) is published for about half of Switzerland. Licence: swisstopo OGD terms, free use including commercial, processing allowed, source "(c) swisstopo" mandatory (the STAC item says "proprietary"; the OGD page is the operative text). The cantonal "DTM-AV" is only mentioned as swissALTI3D's predecessor. Sources: https://www.swisstopo.admin.ch/en/height-model-swissalti3d ; https://data.geo.admin.ch/api/stac/v0.9/collections/ch.swisstopo.swissalti3d ; https://data.geo.admin.ch/api/stac/v0.9/collections/ch.swisstopo.swissalti3d/items?datetime=2026-01-01T00:00:00Z/..&limit=1 ; https://www.swisstopo.admin.ch/en/height-model-swisssurface3d ; https://www.swisstopo.admin.ch/en/height-model-swisssurface3d-raster ; https://www.swisstopo.admin.ch/en/lidar-data-swisstopo ; https://www.swisstopo.admin.ch/en/height-model-swisssurface3d-update-20260217 ; https://www.swisstopo.admin.ch/en/terms-of-use-free-geodata-and-geoservices .

**Swiss cantons (all 26 accounted for).** Method: opendata.swiss CKAN searches (lidar, Terrainmodell, Geländemodell, modèle numérique, modello digitale, MNT, DTM, Laserscanning, Punktwolke) and the geocat.ch API, then each record. opendata.swiss "Open use" terms (commercial use ok, source recommended): https://opendata.swiss/en/terms-of-use . Beat swisstopo on grid: **Geneva** MNT/MNS 2025 at 0.20 m, night lidar 21 Mar-1 Apr 2025, at least 100 pts/m2 (Riegl VQ-1560II-S), ±10 cm vertical, COG 29.5 GB and ImageServer (the 2025 point-cloud record lists no resources, so its download route is unverified): https://raster.sitg.ge.ch/arcgis/rest/services/MNA_TERRAIN_2025_03/ImageServer?f=json ; **Zürich** DTM 2021-22 at 0.25 m, mean 16 pts/m2, whole canton + 500 m buffer, 500 m tiles, point cloud also open, next flight budgeted for 2027: https://maps.zh.ch/download/hoehen/2022/ ; https://www.zh.ch/de/news-uebersicht/mitteilungen/2023/planen-bauen/geoinformation/aktuelle-hoehendaten-fuer-den-kanton-zuerich.html ; **Luzern** DTM 2024 at 0.25 m (issued 2025-02-28, GeoTIFF, STAC API) and point cloud about 16 pts/m2: https://daten.geo.lu.ch/produkt/meta/DTM24025_DS ; **Basel-Landschaft** DTM 2024 at 0.5 m and 0.25 m from a lidar flown 5 Feb-2 Mar 2024 (mean 16 pts/m2, accuracy below ±0.1 m); the metadata cites "Nutzungsbedingungen des Kantons Basel-Landschaft" which were not read, so the licence is unclear: access through the cantonal GeoShop BL (not opened); **Aargau** LiDAR DTM 2024, 0.5 m, 15 cm or better (about equal). The others: Bern's canton DTM is historised and points to swissALTI3D (city of Bern DTM/DOM 2023 at 25 cm exists, city only); Schaffhausen 0.5 m from 2013 (older); Fribourg lidar 2016 (sparser, older); Solothurn and Thurgau re-host swissALTI3D; Vaud's LiDAR 2019 is the swisstopo product; Neuchâtel 2001/2010; Jura contours; Basel-Stadt DTM 2024 as hillshade via WMS/WMTS; Zug hillshade only; Uri, Schwyz, OW, NW, Glarus, Graubünden, Ticino, Valais, AR, AI, St. Gallen: nothing finer found, swisstopo covers it. Sources: https://ckan.opendata.swiss/api/3/action/package_search?fq=organization:amt-fuer-tiefbau-und-geoinformation-fl&rows=100&fl=name,title,metadata_modified (used for FL) ; https://www.vd.ch/territoire-et-construction/cadastre-et-geoinformation/geodonnees/altimetrie-lidar ; https://www.ag.ch/de/verwaltung/dfr/geoportal/geodaten/geodatenliste?rewriteRemoteUrl=/details/AGIS.KAI_LiDARDTM24 .

**Liechtenstein.** Covered by swissALTI3D and swissSURFACE3D (swisstopo names the Principality; the lidar part published 2026-02-17). The local Amt für Tiefbau und Geoinformation "Digitales Terrainmodell (DTM) 2015" (0.5 m grid, 4 pts/m2, flown 8 Apr and 26 Aug 2015, ±0.2 m) is older, sparser and less accurate; the portal geodaten.llv.li failed TLS twice and llv.li answered 403. https://www.swisstopo.admin.ch/en/height-model-swisssurface3d-update-20260217 ; https://ckan.opendata.swiss/api/3/action/package_search?fq=organization:amt-fuer-tiefbau-und-geoinformation-fl&rows=100&fl=name,title,metadata_modified .

**Austria, federal (BEV).** "Digitales Geländehöhenmodell (ALS-DGM) - Höhenraster": 1 m, ±0.50 m generally (larger in high mountains), 55 tiles of 50 x 50 km (GeoTIFF, EPSG:3035, about 12 GB each), annual Austria-wide mosaics (Stichtage 15.09. of 2019, 2021-2025; the Stichtag is the creation date, not the flight date), regions re-flown up to every 10 years; heights EVRF2000 Austria (EPSG:9274). CC BY 4.0, "keinen Einschränkungen", direct download without login (directory listing is 403). BEV publishes no point cloud. The older 10 m DGM Austria is superseded. https://www.bev.gv.at/Services/Produkte/Digitales-Gelaendehoehenmodell/ALS-Hoehenraster.html ; https://data.bev.gv.at/geonetwork/srv/api/search/records/_search ; https://www.data.gv.at/api/hub/search/search?q=ALS%20DTM%20H%C3%B6henraster%201m%20Stichtag&limit=1000 ; https://data.bev.gv.at/download/ALS/DTM/20250915/ALS_DTM_CRS3035RES50000mN2650000E4650000.tif (HEAD).

**Burgenland.** DGM and DOM 0.5 m from the 2019 ALS, ±15 cm position, ±7.5 cm height (1 sigma), "flächendeckend", CC BY 4.0 AT, municipality download; no point cloud found. https://geodaten.bgld.gv.at/de/downloads/hoehenmodelle-orthofotos.html .

**Kärnten.** Verified open: 5 m DGM/DOM (ALS 2006-2015) and 10 m. A new "Laserscanbefliegung (2022-2029)" record (published 2026-10-09) lists Raster and Punktwolke download URLs, but gis.ktn.gv.at timed out on every request (4 tries), so resolution and openness of those products are unverified. https://www.data.gv.at/api/hub/search/search?q=K%C3%A4rnten%20Gel%C3%A4ndemodell&limit=300 .

**Niederösterreich.** Open: DGM 10 m / DOM 10 m (CC BY 4.0); ALS flight blocks 2011/12-2021/22 (22 blocks in the WFS); finer laser products only via the Geoshop (login, stopped). https://noe.gv.at/noe/Open-Government-Data/Geographie_Planung.html ; https://geoshop.noel.gv.at/ .

**Oberösterreich.** DORIS DGM 0.5 m (GeoTIFF and XYZ per municipality, heights EPSG:5778), DOM 0.5 m; latest flight per DKM sheet from the flight-year zip: 2015 1,595; 2016 611; 2017 1,081; 2018 556; 2019 495; 2020 468; 2021 694; 2022 843; 2023 1,994; 2024 905; 2025 992 (and 44 from 2014); CC BY 4.0; no open point cloud. https://e-gov.ooe.gv.at/at.gv.ooe.intramapgem/dop/downloads/41501/41501_DGM_tif.zip (HEAD) ; https://e-gov.ooe.gv.at/at.gv.ooe.dorisdaten/DORIS_Basisdaten/HOEHENMODELL_AKTUELLE_FLUGJAHRE.zip ; https://doris-metadaten.ooe.gv.at/at.gv.ooe.doris.geonetwork_extern/srv/api/records/68CFE491-7CDF-4781-A1F1-603CD0738ABD/formatters/xml .

**Salzburg.** SAGIS DGM 1 m / DOM 1 m by municipality (TIF+TFW, ASCII), ALS epochs: epoch 2 2016-2021, epoch 3 2022-2024 (541 of 1,385 sheets so far); equal to BEV (data feeds BEV); CC BY 4.0; no open cloud. https://service.salzburg.gv.at/ogd/client/showDetail/aa7b0528-70c1-42c4-a03f-87d2d6a452d7 ; https://static.salzburg.gv.at/sagisdaten_download/SAGIS_DATEN/ALS/laserscanabdeckung.zip .

**Steiermark.** Open: DGM/DOM 1 m from ALS 2008-2014 (WCS and 10x10 km ZIPs); a second campaign since June 2022 produces 50 cm DGM/DOM and a LAS 1.4 cloud (mean 8 pts/m2) whose online availability is unverified; the original cloud is a paid "Sonderanfertigung". CC BY 4.0. https://www.landesentwicklung.steiermark.at/cms/beitrag/12803182/142970647/ ; https://landesentwicklung.steiermark.at/cms/beitrag/12877342/145230171 .

**Tirol.** DGM/DOM mosaics 50 cm (GeoTIFF) + 1 m XYZ + contours; first scan 2006-2010, second full scan 2017-2023, ground-point density at least 8 pts/m2; CC BY 4.0, "frei und kostenlos"; sheet-wise download and WCS (50 cm, 5 m); classified point clouds on request (EUR 86.09/h). https://tirol.gv.at/sicherheit/geoinformation/geodaten-tiris/laserscandaten ; https://tirol.gv.at/sicherheit/geoinformationen-karten/freie-laserscandaten-und-orthofotos .

**Vorarlberg.** VOGIS Geländehöhenmodell 2023 at 50 cm (4th scan; also 2017, 2011 at 1 m), CC BY 4.0; only a WMS and the ISO record were found; a direct file download route is unverified. https://vogis.cnv.at/geonetwork/srv/api/records/bd8bc975-a1e9-4374-811f-5773c7a84e33/formatters/xml .

**Wien.** MA 41 DGM 1 m (ESRI ASCII and TIF, 2.5 km blocks), CC BY 4.0 ("kostenfrei"); example block updated 2026-10-31; flight year not found. https://www.wien.gv.at/stadtplanung/geodatenviewer-ogd-daten-herunterladen ; https://www.wien.gv.at/ma41datenviewer/downloads/geodaten/dgm_tif/35_4_dgm_tif.zip (HEAD) .

### 3.6 Italy [it1: national, Piemonte, Valle d'Aosta, Lombardia, Liguria, Trento, Bolzano, Veneto, FVG, Emilia-Romagna; it2: Toscana to Sardegna, Vatican, San Marino]

"Tile-km2" figures are tile counts on the 0.01-degree grid of the PST-A WFS index times a tile area, an upper-bound footprint proxy, not a measured share. The INSPIRE geoportal host did not resolve from here; Italian records were read from the national catalogue RNDT (geodati.gov.it) and from publishers.

**Italy, national.** (a) PST-A lidar DTM / DSM first / DSM last (Ministero dell'Ambiente, MASE): "densità superiore a 1,5 punti per mq", DTM altimetric error below ±15 cm (1 sigma), planimetric ±30 cm, 1 m grids (2 m in Friuli, Emilia-Romagna, Veneto in places), flown 2008-2015 in phases (2008-09 coast and main rivers, 2010-11 extension, 2013-15 secondary rivers and critical zones); river corridors only ("aste fluviali"), not a blanket. The PCN WFS index lists 164,063 grid tiles (160,185 with a resolution value; roughly 150,000 km2 upper bound nationally). CC BY 4.0, bulk-downloadable through the "Download Banca Dati PST" web app (up to 6 GB per request, a JavaScript shell that I did not drive; a portal update on 10 Nov 2026 added extended 1 m coverage). The national WCS lists only 20/40/75 m DTMs; per-region WMS shows images only. Coverage by region (indicator; tiles with lidar / tiles in the region grid, it2): Toscana 42 %, Umbria 58 %, Marche 67 %, Lazio 36 %, Abruzzo 48 %, Molise 86 %, Campania 71 %, Puglia 57 %, Basilicata 35 %, Calabria 90 %, Sicilia 48 %, Sardegna 38 %. (b) TINITALY/1.1 (INGV): 10 m cell, GeoTIFF, whole Italy (193 tile zips), CC BY 4.0, RMSE below 3.5 m (secondary catalogue), keyless WCS 2.0.1. (c) New: a MASE tender (CIG B2BC00915E, over EUR 30 M, won by an e-GEOS-led group) for a national lidar DTM/DSM at 25 cm, 8 cm vertical, at least 8 pts/m2 plains / 6 mountains, flights ending 2026; no page states that any 25 cm tile is published. IGM's DTM is 20 m and sold. Sources: https://gn.mase.gov.it/portale/pst-dati-lidar ; https://gn.mase.gov.it/portale/distribuzione-dati-pst ; https://sim.mase.gov.it/pst/ ; https://www.rivistageomedia.it/geodata/portale-pst-mase-aggiornamento-lidar-interferometria-2026 ; https://gn.mase.gov.it/portale/wcs ; http://wms.pcn.minambiente.it/ogc?map=/ms_ogc/WMS_v1.3/Vettoriali/QU_Lidar.map&service=WMS&request=GetCapabilities ; https://gn.mase.gov.it/portale/piano-straordinario-di-telerilevamento ; https://tinitaly.pi.ingv.it/Download_Area1_1.html ; http://tinitaly.pi.ingv.it/TINItaly_1_1/wcs?service=WCS&request=getCapabilities ; https://gee-community-catalog.org/projects/tinitaly/ ; https://www.mase.gov.it/pagina/cig-b2bc00915e-affidamento-del-servizio-di-realizzazione-del-modello-digitale-del-terreno ; https://www.rivistageomedia.it/survey-and-positioning/al-via-la-realizzazione-del-nuovo-modello-altimetrico-del-territorio-italiano ; https://web.archive.org/web/2026/https://igmi.org/en/descrizione-prodotti/cartografia-digitale/digital-terrain-model-dtm .

**Piemonte.** Regional "RIPRESA AEREA ICE 2009-2011 - DTM 5" (lidar, 5 m, ±0.30 m, ±0.60 m in forest/dense urban, whole region), CC BY 4.0, WCS/WMS and sheet download; the live host timed out (read via the Wayback copy). PST-A 1 m: 11,444 tiles (flights Oct 2008 and 2014). The river-basin authority AdbPo has 1 m river DTMs (DTM Po 2024, CC BY 4.0), corridors only. No 2022+ regional lidar found. https://web.archive.org/web/2026/https://www.geoportale.piemonte.it/geonetwork/srv/api/records/r_piemon:224de2ac-023e-441c-9ae0-ea493b217a8e ; https://geodati.gov.it/resource/id/adbpo:0abd7059-6d4b-48a3-8da8-18fe41 .

**Valle d'Aosta.** Regional lidar DTM/DSM: DTM 2005/2008 aggregate at 2 m for the whole region (2005 Dora valley at 0.5 m degraded to 2 m in the aggregate), DTM 2022 Dora Baltea 0.5 m (Riegl LMS-Q680i, about 12 raw pts/m2, flown 15 May 2022, Pont-Saint-Martin to Saint-Pierre only). CC BY 4.0 "per qualsiasi fine, anche commerciale". GeoDownload app, optional login. https://geoportale.regione.vda.it/download/dtm/ ; https://metadati.partout.it/metadata_documents/DTM_2022_DORA_ORTOMETRICA.pdf ; https://metadati.partout.it/metadata_documents/DTM_2005_2008_ORTOMETRICA.pdf ; http://metadati.partout.it/metadata_documents/CC_BY_DTM_v2.pdf ; https://mappe.regione.vda.it/pub/geonavitg/geodownload.asp?carta=DTM0508 .

**Lombardia.** "DTM 5X5 (ed. 2015)": 5 m built from the regional topographic database (about 80 % contours, spot heights, breaklines) plus 1 m lidar along watercourses; accuracy 0.30 m (urban) to 2 m (forest); whole region; CC BY 4.0; ZIP 3,085,490,651 bytes (HEAD), WMS. The regional lidar coverage service shows flights 2002-03, 2005-06, 2014 (CNR-IRPI) and says some lidar is available only to public administrations (RESTRICTED); open lidar is via PST-A (8,563 tiles, Jun 2009). https://www.geoportale.regione.lombardia.it/en/note-legali ; https://geodati.gov.it/resource/id/r_lombar:975979a7-68e8-4524-b759-0db4d8284939 ; https://www.cartografia.servizirl.it/download/DTM5_RL.zip (HEAD) ; https://www.regione.lombardia.it/ambiente-e-territorio/infrastruttura-dati-territoriali-idt/consultazione-e-distribuzione-delle-foto-aeree-e-dei-rilievi-lidar .

**Liguria.** PST-A 1 m (5,799 tiles, flight Oct 2008) is the finest open source; the regional "DTM ed. 2023" is 5 m from vector data (source accuracy 0.6 m). CC BY 4.0, WCS and download viewer. https://geodati.gov.it/resource/id/r_liguri:D.2056 ; http://wms.pcn.minambiente.it/ogc?map=/ms_ogc/WMS_v1.3/servizi-LiDAR/LIDAR_LIGURIA.map&service=wms&request=getCapabilities&version=1.3.0 .

**Trento (Provincia autonoma).** "LiDAR PAT 2014 integrato con voli 2018" (+ Campolongo 2011): DTM 0.5 m (ASC, XYZ; 5 m and 10 m mosaics), 10 ground pts/m2 below 1800 m and 4 above, 2018 update 5 pts/m2, ±25 cm altimetric (2 sigma); raw LAZ also published; CC BY 4.0 (record) / CC BY 2.5 (Mapterhorn attribution list https://download.mapterhorn.com/attribution.json); open directory of 25,201 ASC tiles (about 194 GB). Beats PST-A (224 tiles, Sep 2008). No 2021/2023 flight found. https://geodati.gov.it/resource/id/p_TN:79c6cdf0-c73a-46ba-870c-704bf4e27ac3 ; https://www.provincia.tn.it/News/Approfondimenti/Rilievo-Lidar-del-territorio-della-Provincia-di-Trento ; https://siatservices.provincia.tn.it/stemdata/2014_lidar_dtm_asc/ .

**Bolzano / Alto Adige (Provincia autonoma).** DTM 0.5 m "delle aree insediative" (record created 2013-11-30), DTM 2.5 m (whole province, 2006), DTM Adige 20 cm (flown 11, 12, 17 Dec 2024 and 24 Jan 2025, between Tel and Salorno), glacier DSM 0.5 m (2016/17, 2023), lidar point-cloud coverage grids (download path not verified); all CC0 with "nessuna limitazione". Keyless WCS 2.0.1. https://geodati.gov.it/resource/id/p_bz:Elevation:DigitalTerrainModel-0.5m ; https://geodati.gov.it/resource/id/p_bz:Elevation:DigitalTerrainModel-2.5m ; https://geoservices9.civis.bz.it/geoserver/ows?service=WCS&version=2.0.1&request=GetCapabilities ; https://geonetwork1.civis.bz.it/geonetwork/srv/api/records/p_bz:Elevation:DigitalTerrainModel-EtschAdige-0.2m-2024 .

**Veneto.** "DTM Lidar 5 m" (announced 12 Jul 2024): mosaic of over 40 regional campaigns over about 20 years (most originals 1 m, newest wins), lidar over about 15,400 km2 (over 80 % of the region); IODL 2.0; download by province/commune. PST-A 1 m (8,242 tiles) is finer along rivers. https://geodati.gov.it/resource/id/r_veneto:c0101071_Lidar5m ; https://idt2.regione.veneto.it/nuovo-dtm-derivato-da-dati-lidar/ ; https://idt2.regione.veneto.it/idt/downloader/download ; https://www.asita.it/wp-content/uploads/2024/12/poster_vincenzi_asita.pdf ; https://www.dati.gov.it/iodl/2.0/ .

**Friuli-Venezia Giulia.** "Rilievo LIDAR RAFVG 2017-2020": point clouds (LAZ 1.4, classified; mean 16 pts/m2 up to 1000 m, 10 above; Riegl LMS-Q780) and DTM / DSM first / DSM last / DBM at 0.5 m for the whole region; IRDAT states IODL 2.0 as the regional standard, records say "Dato pubblico"; Eagle.fvg guest app (file-level download not tested). Beats PST-A (2 m, 2010). https://geodati.gov.it/resource/id/r_friuve:m10856-cc-i11826 ; https://geodati.gov.it/resource/id/r_friuve:m10856-cc-i11647 ; https://eaglefvg.regione.fvg.it/eagle/main.aspx?configuration=guest ; https://web.archive.org/web/2026/https://irdat.regione.fvg.it/consultatore-dati-ambientali-territoriali/detail/irdat/dataset/11826 .

**Emilia-Romagna.** DTM 0.5 m RER 2022 (pilot, 1,433 km2, 4 pts/m2), RER 2023/24 (9,240 km2, at least 8 pts/m2, ±8 cm vertical, flown Mar 2023-Jan 2025), RER 2025/26 (in progress; DTM WCS and index published 3-4 Sep 2026); aim "rilievo completo dell'intero territorio regionale"; plus Bologna 2023 and Modena 2024 city lidar, river DTMs 1 m. CC BY 4.0. Keyless WCS per epoch; point clouds not bulk-offered. https://geoportale.regione.emilia-romagna.it/approfondimenti/rer23_24 ; https://geoportale.regione.emilia-romagna.it/approfondimenti/prodotti-lidar-e-ortofoto ; https://servizigis.regione.emilia-romagna.it/wcs/dtmrer2022?request=getcapabilities&service=wcs ; https://servizigis.regione.emilia-romagna.it/wcs/dtmrer2023_24?service=wcs&request=getcapabilities&version=1.0.0 ; https://servizigis.regione.emilia-romagna.it/wcs/dtmrer2025_26?service=wcs&request=getcapabilities&version=1.0.0 ; https://geodati.gov.it/resource/id/r_emiro:2026-09-03T160720 . NON-GOVERNMENTAL (academic): a Ca' Foscari paper reads RER 2022 via WCS and compares it with PST-A: https://iris.unive.it/retrieve/5323c7fc-6ba5-4e6c-beac-b08077a52bb6/Abballe%20et%20al.%202025_Novel%20Airborne%20LiDAR-Derived%20Digital%20Terrain%20Models%20in%20the%20Emilia-Romagna%20Region%20%28Italy%29.pdf .

**Toscana.** Regional "LiDAR 2021 non di proprietà RT" (owned by Italian Remote Sensing S.r.l.; flights 2019, 2020, 2021; DTM+DSM 1 m, designed for 1.5 pts/m2, Riegl VQ780ii): "(solo per enti pubblici)" so a personal offline map is NOT allowed; the tile-index shapefile is CC BY. Open alternatives: Regione Toscana DSM 2021 (1 m, photogrammetric surface model, CC BY, not terrain); Comune di Firenze "MDT LIDAR 2017" (CC BY 4.0, city only). PST-A covers 42 % of the grid. https://www502.regione.toscana.it/geoscopio/geometa/html/r_toscan_261ec1b2-bf25-4973-b853-ac08e060ec62.htm ; https://www502.regione.toscana.it/geonetwork/srv/api/records/r_toscan:f3ff89ee-49fa-4487-89fe-c69ae58ecd0c/formatters/xml ; https://www502.regione.toscana.it/geonetwork/srv/api/records/r_toscan:c5c907bf-50cc-4a76-8787-abc547509c8d/formatters/xml ; https://dati.toscana.it/api/3/action/package_show?id=dtm-2017 .

**Umbria.** PST-A only (1 m, 2008-2014, 58 % of grid). The regional UmbriaGeo DTM is built from IGM 1:25,000 contours (1997), far coarser; UmbriaGeo hosts timed out, Wayback copies read. https://web.archive.org/web/2026/https://umbriageo.regione.umbria.it/catalogostazioni/catalogo.aspx?cd=149 ; https://web.archive.org/web/2026/https://umbriageo.regione.umbria.it/articoli/download-dati-cartografici-000 .

**Marche.** PST-A 1 m (67 % of grid). Regional DTM from CTR 1:10,000 (flight 1999/2000): 20 m / 100 m, level 2 (±2 m open terrain, ±7 m dense vegetation), CC BY 4.0; no regional lidar. https://www.regione.marche.it/Portals/0/Paesaggio_Territorio_Urbanistica/Cartografia/Metadati/Relazione_DTM.pdf ; https://web.archive.org/web/2026/https://www.regione.marche.it/Regione-Utile/Paesaggio-Territorio-Urbanistica-Genio-Civile/Cartografia-e-informazioni-territoriali/Repertorio .

**Lazio.** PST-A 1 m (36 % of grid). Regional: "Digital Terrain Model Passo 5 mt v. 2002/2003" (DXF per province, from the CTRN), CC BY 4.0; searches for lidar/DSM return 0. https://geoportale.regione.lazio.it/api/v2/resources?search=dtm&page_size=30 .

**Abruzzo.** PST-A 1 m (48 %). The Cartanet portal lists a "Modello Digitale Terreno" whose grid, vintage and licence are not readable (JavaScript shell). http://geoportale.regione.abruzzo.it/Cartanet/ .

**Molise.** PST-A 1 m (86 %, highest after Calabria). The regional DTM in RNDT is a 40 m WMS derived from SRTM 90 m: worse than the global products. https://gn.mase.gov.it/portale/pst-dati-lidar .

**Campania.** Regional "CTR2020 - DTM5K": 5 m float32, whole region, photogrammetric from the AGEA 2020 flights, CC BY 4.0, ArcGIS ImageServer, WMS, WCS; PST-A + MIADRA offers 1 m (2008-13) on 71 % of the grid. https://geomaps.regione.campania.it/server/rest/services/Amministrativo/CTR_DTM5m/ImageServer?f=json ; https://sit.regione.campania.it/carta-tecnica-regionale .

**Puglia.** PST-A + MIADRA 1 m (57 %). Regional SIT Puglia DTM: 8 m photogrammetric (2006 flights), vertical accuracy 1 m, IODL 2.0 (terms not read); the download page redirects to a login gateway (stopped). https://gn.mase.gov.it/portale/distribuzione-dati-pst .

**Basilicata.** PST-A 1 m covers only 35 % of the grid; RSDI Basilicata DTM/DSM 5 m (2013, photogrammetric, IODL 2.0) fills the gap (host timed out; liveness unconfirmed). https://gn.mase.gov.it/portale/distribuzione-dati-pst .

**Calabria.** PST-A + MIADRA 1 m on 90 % of the grid (flown 2008-2014). The regional geoportal lists only CTR 2007/2008 and flights; "Lidar Calabria 2022-23" was not found. https://gn.mase.gov.it/portale/pst-dati-lidar .

**Sicilia.** SITR ImageServers modelli_digitali/mdt_2013 (2 m float32, whole island, ATA 2012-13 flight, photogrammetric not lidar, EVRF2019 heights) and mdt_2008; CC BY 4.0 per the SITR licence page (whether it covers the ImageServer is not stated); exportImage and identify answered. PST-A 1 m covers 48 %. https://map.sitr.regione.sicilia.it/gis/rest/services/modelli_digitali?f=json ; https://www.sitr.regione.sicilia.it/licenza-duso/ ; https://www.sitr.regione.sicilia.it/riprese-aereee/volo-anni-2012-2013/ .

**Sardegna.** Regional 1 m lidar DTM/DSM: coast 2008, inland urban centres flight 2013 (±15 cm vertical), rivers 2008-09 and Gallura (re-projected PST-A, CC BY-SA 3.0 IT for the original), plus a 10 m full-island DTM (2011); CC BY 4.0 for the regional datasets. The 1 m mosaic is WMS only (images); WCS not offered; the download navigator is a JavaScript app. No flight after 2013 found (the "2022" layers are orthophotos). https://webgis.regione.sardegna.it/geoserverraster/ows?service=wms&version=1.3.0&request=GetCapabilities .

**Vatican City State.** Nothing published by the Governatorato found. Italian PST-A 1 m tiles (contract 140, flown 2008-05-02, regione=Lazio) cover tiles at lon 12.45-12.46, so the east of the Vatican; the western part (west of 12.45 E) has no tile; the Vatican boundary itself was not fetched. http://wms.pcn.minambiente.it/ogc?map=/ms_ogc/wfs/QU_Lidar.map&service=WFS&version=1.1.0&request=GetFeature&typeName=EL.QUADRO.UNIONE.LIDAR.0101&PROPERTYNAME=regione,contratto,data,tavola&BBOX=41.88,12.43,41.92,12.47,urn:ogc:def:crs:EPSG::4326 .

**San Marino.** The Ufficio Tecnico del Catasto e Cartografia page mentions periodic aerophotogrammetric surveys and a GIS but no DTM, lidar, licence or geoportal; the PST-A grid has 95 tiles in an approximate box (43.89-44.00 N, 12.39-12.53 E), none tagged San Marino, so coverage of the Republic is not established. Emilia-Romagna's newer lidar may reach across the border (not checked). https://www.gov.sm/pub1/GovSM/Dipartimenti/Dipartimento-Territorio-e-Ambiente/Ufficio-Tecnico-del-Catasto-e-Cartografia.html .

### 3.7 Spain [es1: national, Andalucía, Aragón, Asturias, Baleares, Canarias, Cantabria, Castilla-La Mancha, Castilla y León, Catalunya; es2: Valencia, Extremadura, Galicia, La Rioja, Madrid, Murcia, Navarra, País Vasco, Ceuta, Melilla]

**Spain, national (IGN/CNIG, PNOA).** PNOA-LiDAR third coverage (flown 2021 for Catalunya and 2023 to Mar 2026 elsewhere): at least 5 pts/m2 (1x1 km LAZ, RGBI-coloured, classification levels NPC01 automatic / NPC02 basic edit / NPC03 advanced; 621,035 files), RMSE z at most 10 cm (specification page), MDT50cm and MDS50cm as COG on 1:5000 sheets (MDT50cm 28,963 files, MDS50cm 14,280; "Cobertura por completar"). Status page header "ESTADO ACTUAL (SEPTIEMBRE 2026)": all 17 communities plus Ceuta and Melilla have been flown; MDT50cm v1 is published for Andalucía, Canarias, Cantabria, Catalunya, Comunitat Valenciana, Extremadura, Illes Balears, Murcia, Madrid, País Vasco, Navarra, La Rioja, Ceuta and Melilla; not yet (forecast 2nd half 2026) for Castilla y León, Castilla-La Mancha (East and West), Asturias, Galicia and Aragón. Older: second coverage (2015-2021, 0.5-4 pts/m2, 322,264 files) with MDT02 (2 m COG, 8,308 sheets); first coverage (2008-2015, 0.5 pts/m2) with MDT05 5 m, MDT25, MDT200. There is no 1 m product. Lot tables give RMSE z 0.15-0.20 m (2nd coverage). Datum orthometric (Alicante MSL, EPSG:5782; Balears and Canarias use local MSL). Licence: "licencia de uso, compatible con CC-BY 4.0" (Orden FOM/2807/2015; attribution only; commercial use allowed); metadata "CC BY 4.0 scne.es". Access: file-by-file POST form at the Centro de Descargas (FAQ: no registration for up to 20 downloads, login from the 21st, up to 40,000 files per session when registered); the form's JavaScript flags WebDriver browsers as "BOT_DETECTED" (read in the page source, not tested), so scripted pulls from the CdD may be blocked; the INSPIRE WCS 2.0.1 (keyless, fees none) offers only 5 m and coarser (Elevacion{4258,4326,25830,4083}_{5,25,200,500,1000}); unpublished data on request with costs. Note: the MDT05/25/200 sheets for Ceuta, Melilla, Alborán, Gibraltar and Andorra are photogrammetric, not lidar. Sources: https://centrodedescargas.cnig.es/CentroDescargas/lidar-tercera-cobertura ; https://centrodedescargas.cnig.es/CentroDescargas/modelo-digital-terreno-mdt50cm ; https://centrodedescargas.cnig.es/CentroDescargas/modelo-digital-superficies-mds50cm-tercera-cobertura ; https://centrodedescargas.cnig.es/CentroDescargas/modelo-digital-terreno-mdt02-segunda-cobertura ; https://centrodedescargas.cnig.es/CentroDescargas/lidar-segunda-cobertura ; https://centrodedescargas.cnig.es/CentroDescargas/modelos-digitales-elevaciones ; https://pnoa.ign.es/web/portal/pnoa-lidar/tercera-cobertura ; https://pnoa.ign.es/web/portal/pnoa-lidar/segunda-cobertura ; https://pnoa.ign.es/web/portal/pnoa-lidar/primera-cobertura ; https://pnoa.ign.es/web/portal/pnoa-lidar/especificaciones-tecnicas ; https://pnoa.ign.es/web/portal/pnoa-lidar/productos-a-descarga ; https://pnoa.ign.es/web/portal/pnoa-lidar/modelo-digital-del-terreno ; https://idee.es/csw-inspire-idee/static/api/records/spaignMDT02 ; https://servicios.idee.es/wcs-inspire/mdt?REQUEST=GetCapabilities&SERVICE=WCS ; https://centrodedescargas.cnig.es/CentroDescargas/faqs ; https://centrodedescargas.cnig.es/CentroDescargas/politica-datos ; https://datos.gob.es/apidata/catalog/dataset/e00125901-spaignmdt.json .

**Andalucía.** REDIAM "MDE del PNOA-LiDAR de Andalucía 2020-21 (1x1 m)" (COG, from the PNOA 2nd-coverage cloud: 1.5 pts/m2, LAZ 2x2 km, RMSE z 20 cm; flown Jul 2020-May 2021), CC BY 4.0, public Nextcloud share; the only gain over CNIG's 2 m MDT02 is the grid. The national third coverage (flown Apr-Sep 2024, MDT50cm v1 published) is finer and newer. https://portalrediam.cica.es/geonetwork/srv/api/records/fb916b4f-76c4-4d8f-a3d2-4fd3641e509d ; https://portalrediam.cica.es/geonetwork/srv/api/records/3d2a3ae0-2819-480a-8754-1b3a92805419 ; https://portalrediam.cica.es/geonetwork/srv/api/records/7c94b2b2-52e3-4351-b156-44094f7fe953 .

**Aragón.** IGEAR/ICEARAGON hold only PNOA re-distributions: LIDAR 2010 (0.5 pts/m2), 2016 (1 pt/m2), 2021 tile grid, DEMs MDE 25 m and MDT2012 5 m derived from IGN's MDT; CC BY 4.0. National third coverage flown Jun-Sep 2023 (NPC02 published; MDT v1 not yet, v2 forecast 2nd half 2026). https://icearagon.aragon.es/descargas.jsp ; https://icearagon.aragon.es/geonetwork/srv/spa/q?any=lidar&fast=index&_content_type=json&resultType=details&to=30 .

**Asturias.** The regional "Elevaciones del Principado de Asturias" is contour lines, spot heights and a hillshade (no DEM values); licence of regional data unclear (the SITPA download page is dynamic and ideas.asturias.es timed out). National: second coverage lot AST (Nov 2020-Jul 2021, 1 pt/m2) and third coverage flown May 2024-Feb 2026 (NPC01 published, MDT v1 forecast 2nd half 2026). https://datos.gob.es/apidata/catalog/dataset/a03002951-elevaciones-asturias.json ; https://web.archive.org/web/2026/https://ideas.asturias.es/en/centro-de-descargas .

**Illes Balears.** IDEIB "Altimetria": MDT/MDS LIDAR 2014 at 2x2 m (+ slope, aspect, hillshade), served as an ArcGIS MapServer (a raw-grid download was not confirmed), licence CC BY-SA ("Reconeixement-Compartir Igual", "Es permet la publicació i descàrrega per Internet"). National beats it: PNOA 2nd coverage BAL (Nov 2019-Feb 2020, 1 pt/m2) and 3rd coverage (flown Mar-May 2024, NPC03 and MDT/MDS v1 published). The islands' vertical datum is local Mediterranean MSL. https://ideib.caib.es/geoserveis/rest/services/public/GOIB_Altimetria_IB/MapServer/layers?f=pjson ; https://ideib.caib.es/geoserveis/rest/services/public/2021BTIB5000/MapServer/32/iteminfo?f=pjson .

**Canarias.** Open regional data: MDT 25 m (7 islands) and a post-eruption La Palma LAZ (708,246,149 bytes by HEAD, modified 2024-09-18) on opendata.sitcan.es (CC-by 3.0; datos.canarias.es says CC-by 4.0); GRAFCAN's lidar (average 1 pt/m2) is visualisation-only in the viewer and its MDT/lidar products are sold under the "Licencia de Uso de GRAFCAN" (not read). National third coverage Canarias flown Nov 2023-Jan 2024 (NPC02 and MDT/MDS v1 published) is better. https://www.grafcan.es/servicios/lidar-2/ ; https://www.grafcan.es/servicios/ventas-y-descargas/ ; https://opendata.sitcan.es/api/3/action/package_search?q=lidar&rows=20 ; https://opendata.sitcan.es/notice ; https://datos.canarias.es/portal/aviso-legal-y-condiciones-de-uso ; https://opendata.sitcan.es/upload/244/LDR/laz/244_LDR_LP.laz (HEAD).

**Cantabria.** Regional ArcGIS MapServer with 5 m DTMs (2001-2012 sources) and a 1 m DSM from the 2018/19 campaign (images/identify; raw-grid download not confirmed; no explicit licence text read). National third coverage flown Sep-Dec 2023, NPC03 and MDT/MDS v1 published. https://geoservicios.cantabria.es/inspire/rest/services/MDT/MapServer/layers?f=pjson ; https://geoservicios.cantabria.es/inspire/rest/services/MDT/MapServer?f=pjson .

**Castilla-La Mancha.** No regional DTM beyond a viewer "MDT LIDAR COB2 CLM" (2 m from lidar 2016-2020, i.e. PNOA 2nd coverage); the open-data hub returns 0 hits for lidar/MDT. National third coverage East Apr 2025-Jan 2026, West Jul-Oct 2025 (NPC01; MDT v1 forecast 2nd half 2026). https://cartografia.castillalamancha.es/descargas-datos%20geograficos ; https://www.coigt.com/noticia/3435 .

**Castilla y León.** Regional "MDT CyL 2008-2011" 5 m (SW from stereo-correlation RMSE about 80 cm; others from the 1st-coverage lidar, RMSE about 20 cm), "CC BY 4.0 scne.es"; national third coverage flown Sep 2025-Mar 2026 (NPC01; MDT v1 forecast 2nd half 2026). https://idecyl.jcyl.es/geonetwork/srv/api/records/SPAGOBCYLCITDTSELMDT0811 .

**Catalunya (ICGC).** Territorial LiDAR third coverage v3.1 (2021-2023): at least 8 pts/m2, 65 % of 1x1 km sheets above 10, LAS 1.4, ASPRS classes, 159 flight days, altimetric RMSE about 6 cm in flat low-vegetation areas, CC BY 4.0. Elevation models (Sept 2026 update): DTM 0.5 m, 1 m, 5 m for all Catalonia, DTM 25 cm for the Barcelona metropolitan area, DSM 1 m (25 cm for AMB), older 2 m 2008-2011 DTM; EPSG:25831. Open directory listings, no login; the 50 cm DTM is one 421,964,204,579-byte GeoTIFF (HEAD), plus VRTs and terrain-RGB. CNIG also carries Catalunya third-coverage tiles at "5 ptos/m2". https://www.icgc.cat/en/Geoinformation-and-Maps/Data-and-products/Elevations/Territorial-elevations/Territorial-LiDAR ; https://www.icgc.cat/en/Geoinformation-and-Maps/Data-and-products/Elevations/Territorial-elevations/Elevation-models ; https://www.icgc.cat/en/ICGC/News/We-have-completed-third-LiDAR-coverage-Catalonia ; https://datacloud.icgc.cat/datacloud/model-elevacions-terreny/tif_unzip/ ; https://datacloud.icgc.cat/datacloud/lidar-territorial/ ; https://catalegs.ide.cat/geonetwork/srv/api/records/lidar-territorial-v3r1-2021-2023 .

**Comunitat Valenciana (ICV).** 1 m MDT GeoTIFF per province reprocessed from the PNOA 2nd-coverage clouds (Valencia 2015, Alicante 2016, Castellón 2017; clouds 0.5-1 pts/m2), LAZ 2x2 km, CC BY 4.0, per-sheet download; ICV has not republished the 2023-24 cloud. National third coverage flown Aug 2023-Jun 2024 (NPC02, MDT/MDS v1 published). https://dadesobertes.gva.es/api/3/action/package_search?q=MDT&rows=100 ; https://dadesobertes.gva.es/api/3/action/package_show?id=lidar-2017-de-1-punto-por-metro-cuadrado-de-la-provincia-de-castellon ; https://dadesobertes.gva.es/api/3/action/package_search?q=lidar&rows=100&fl=name,title .

**Extremadura.** No own lidar/MDT: the SITEX download centre links to IGN; CC-BY. National third coverage flown Jun-Sep 2023 (NPC02, MDT/MDS v1 published). https://sitex.juntaex.es/SITEX/centrodescargas/view/5 ; https://sitex.juntaex.es/SITEX/centrodescargas/view/6 .

**Galicia.** IET: manually classified PNOA 2015-16 clouds (8,106 files, 1.2 TB; 0.5 pts/m2, cleaner classes but not denser), MDT 2 m from PNOA 2009-10 as an ArcGIS ImageServer; Xunta legal notice allows reuse with source. National third coverage flown May 2024-Feb 2026 (NPC01; MDT/MDS v1 forecast 2nd half 2026). https://web.archive.org/web/2026/https://mapas.xunta.gal/es/utilidades-y-publicaciones/actualidad/nueva-cobertura-lidar-2015-2016 ; https://ideg.xunta.gal/servizos/rest/services/Elevacions/Mdt2_PNOA09_10/ImageServer?f=pjson ; https://web.archive.org/web/2026/https://mapas.xunta.gal/es/aviso-legal .

**La Rioja.** Regional lidar flight Sept 2016 (2 pulses/m2, 2x2 km blocks) with DTM collections 1x1, 2x2, 5x5 m over the whole community (5,034 km2), RMSE xy at most 20 cm, CC BY 4.0, OGC services; all four larioja.org hosts answered 403 (Cloudflare) or 522 (not bypassed). National third coverage flown Jul-Sep 2024 (NPC01, MDT v1 published). https://iderioja.github.io/doc_lidar_2016_processing_en/processing/introduction/ ; https://iderioja.github.io/doc_lidar_2016_processing_en/dtm_dsm/digital_terrain_models/ ; https://iderioja.github.io/doc_lidar_2016_processing_en/license/ .

**Madrid.** Comunidad: LiDAR flights 2010 and 2016 (1 pt/m2), "CC BY 4.0 scne.es". Ayuntamiento de Madrid: photogrammetric MDT with 10 cm grid, RMSE z 0.40 m, updated every four months, plus a partial F1-circuit lidar (about 60 pts/m2, flown 2026-06-13). National third coverage flown Mar-May 2024 (NPC02, MDT/MDS v1 published). https://idem.comunidad.madrid/catalogocartografia/srv/spa/q?any=lidar&_content_type=json&fast=index&resultType=details&to=40 ; https://idem.comunidad.madrid/catalogocartografia/srv/spa/q?any=terreno&_content_type=json&fast=index&resultType=details&to=40 ; https://datos.madrid.es/pages/condiciones-generales-ayuntamiento-de-madrid .

**Región de Murcia.** No regional elevation dataset found (CSW and CKAN searches return nothing relevant; sitmurcia.carm.es serves a CAPTCHA, stopped). National third coverage flown Aug 2023-May 2024 (NPC02, MDT/MDS v1 published). https://datosabiertos.regiondemurcia.es/api/3/action/package_search?q=lidar&rows=30 .

**Navarra.** Own repository filescartografia.navarra.es: LiDAR 2024 (Optech Galaxy T2000, 5 pts/m2, AI-assisted classification, 1x1 km LAZ, 12,428 tiles about 1.1 TB) with MDT/MDS 50 cm (ASCII grid, 401 files, about 141 GB) and 2 m; LiDAR 2017 (Leica SPL100 single-photon, 10 pts/m2 per the lidar readme, 14 per the MDE readme and the PNOA table, RMSE z 0.15 m) with 50 cm and 2 m; 2011-12 (1 pt/m2). MDT orthometric (REDNAP), clouds ellipsoidal, EPSG:25830. CC BY 4.0; keyless WCS (MDT_2M 2024, MDT_2M_VE2017). https://filescartografia.navarra.es/6_MDE/_leeme.txt ; https://filescartografia.navarra.es/5_LIDAR/_leeme.txt ; https://filescartografia.navarra.es/6_MDE/ ; https://idena.navarra.es/ogc/wcs?service=WCS&request=GetCapabilities ; https://datos.gob.es/apidata/catalog/dataset/a15002917-modelo-digital-del-terreno-mdt-procedente-de-lidar-resolucion-2-metros-ano-2024.json .

**País Vasco / Euskadi.** Gobierno Vasco MDT 1 m "MAX_ACTUALIDAD" 2017 (built from 2008/2012 lidar improved with the 2017 flight: 23 May-11 Oct 2017, 2.2 pts/m2, 500 m sheets, EGM08_REDNAP), keyless WCS holding 2008-2017 MDT/MDS, bulk 1 m MDT zip in 15 parts (about 15.5 GB); use "permitido ... si se menciona la autoría". The lidar directory lists only 2008, 2012 and 2017: nothing from the 2024 flight. Gipuzkoa (b5m): airborne lidar 2005, 2008, 2012, 2017 (1 km tiles), later folders are mobile mapping; CC BY 4.0. Bizkaia and Araba: no own lidar/MDT dataset found (catalogues JavaScript-only, opendatabizkaia.eus timed out). National third coverage flown Jul-Sep 2024 (NPC01, MDT v1 published). https://datos.gob.es/apidata/catalog/dataset/e0dat0002-modelo-digital-del-terreno-mdt-de-1m-de-la-comunidad-autonoma-del-pais-vasco-ano-2017.json ; https://www.geo.euskadi.eus/WCS_KARTOGRAFIA?request=getcapabilities&service=wcs ; https://www.geo.euskadi.eus/lidar/DatosDescarga/LIDAR/ ; https://www.idee.es/csw-codsi-idee/srv/api/records/md_ideeu_mdt_lidar_2017_es/formatters/xml ; https://b5m.gipuzkoa.eus/lidar/ ; https://b5m.gipuzkoa.eus/web5000/en/legal-information ; https://www.bizkaia.eus/es/web/geobizkaia/recursos ; https://geo.araba.eus/es/web/geo/centro-de-descargas .

**Ceuta and Melilla.** Both are in the PNOA: second coverage CEU (flown May 2021, 1.5 pts/m2, RMSE xy 0.3 / z 0.15 m, Optech Galaxy T2000) and MEL (Oct 2020, 1.5 pts/m2, Riegl VQ-1560i-DW); third coverage flown Sep 2024 with NPC02 and MDT/MDS 50 cm v1 published; CC BY 4.0. No Ciudad Autónoma IDE found (hosts timed out or did not resolve). https://pnoa.ign.es/web/portal/pnoa-lidar/tercera-cobertura ; https://pnoa.ign.es/web/portal/pnoa-lidar/segunda-cobertura .

### 3.8 Portugal, Malta, Andorra, Monaco [pt; Monaco and Andorra coverage checks by lead]

**Portugal, mainland.** DGT "LiDAR de Portugal continental" (PRR-funded, first national airborne lidar), served by the Centro de Dados Geográficos on INCD infrastructure: classified LAZ (average 10 pts/m2, 9 classes, RGB+NIR), MDT (DTM) and MDS (DSM) at 0.5 m and 2 m (Float32 GeoTIFF, 1 km tiles, EPSG:3763), accuracy 30 cm planimetric / 10 cm altimetric (SNIG metadata; the vertical datum is not stated). Flown Apr 2024-Jul 2025 in four lots; newest STAC item datetime 2026-06-29. The STAC lists 91,202 DTM-50cm tiles (91,193 for MDT-2m and LAZ), about one per km2 in five spot boxes (Minho 1,190; Porto 1,105; Trás-os-Montes 1,493; Lisbon 428; Algarve 1,201), so the whole mainland (the collection description's "Zona Noroeste ainda indisponível" is stale). Licence "CC-BY-4.0", "Acesso público sem restrições". Access: the STAC API browses and searches without login; the item asset links are opaque URLs under cdd.dgterritorio.gov.pt/dgt-be/v1/download/<hash> that redirect to a Keycloak login (a third-party tutorial says a free account is granted immediately; I did not sign up); the storage object returns 403. Typical sizes: DTM-50cm about 19 MB, DTM-2m about 1.2 MB, LAZ about 154 MB. Older: coastal MDT 1 m 2011 (400 m strip, on request); CIGeoE is a shop; Lisbon's portal returned a Cloudflare challenge. NON-GOVERNMENTAL: OpenDEM's note "Portugal 0.5 m DTM and DSM ... Registration required" confirms the login. https://cdd.dgterritorio.gov.pt/dgt-be/v1/collections ; https://dgt-be.a.incd.pt:8081/collections/MDT-50cm/items?limit=2 ; https://snig.dgterritorio.gov.pt/rndg/srv/api/records/077a8c94-8b46-4a8a-8796-0d7fc4662f0c/formatters/xml ; https://cdd.dgterritorio.gov.pt/dgt-be/v1/collections/MDT-50cm/items/MDT-50cm-205466-07-2025 ; https://www.generic-mapping-tools.org/GMTjl_doc/tutorials/dgt_lidar/tut_dgt_lidar.html ; https://snig.dgterritorio.gov.pt/rndg/srv/api/records/b93228d48b794d4f8c78759409387c8f/formatters/xml ; https://www.igeoe.pt/ ; https://opendem.info/ .

**Azores.** "Cobertura Aérea LiDAR" of all nine islands (Governo dos Açores, SRAAC; contractor Municipia, tender CP 50/SRAAC/2022): 15 pts/m2 (Riegl VQ-780II), LAZ tiles 1.6 x 1.0 km, DTM (MDE) and DSM (MDS) at 1, 5 and 10 m (Float32), RMSEz 9.3 cm (São Miguel), datum Ponta Delgada (EPSG:6187) with GeoidAz2014 for the central group (other groups differ), flown May-Oct 2024 (São Miguel), items 2024-09 to 2026-05. ACORES-MDE-1 has 1,820 tiles; probes cover all nine islands. CC BY 4.0, "Acesso público sem restrições"; same DGT login for file download, and the CDD front end marks the Azores collections `visibility: hide`. The older regional 10 m MDT per island (2007 flights, "Acesso e uso limitado") is superseded; a 2026-04-07 news piece says the lidar "será tornado público brevemente". https://sma.idea.azores.gov.pt/geonetwork/srv/api/records/291673dc-2636-4d9f-9917-4434a5073588/formatters/xml ; https://dgt-be.a.incd.pt:8081/collections/ACORES-MDE-1/items?limit=1 ; https://cdd.dgterritorio.gov.pt/dgt-be/v1/collections/ACORES-MDE-1/items/2024_SMG_PDL_31-2-3-4_01_MDE ; https://sma.idea.azores.gov.pt/geonetwork/srv/api/records/94c6d92d-c986-461f-b822-4fdaa4b80ca4/formatters/xml ; https://observador.pt/2026/04/07/governo-dos-acores-lanca-concurso-para-cartografia-topografica-vetorial-de-elevado-detalhe/ .

**Madeira.** DROTe "Modelo Digital do Terreno (MDT) do Arquipélago da Madeira": photogrammetric (Vexcel UltraCam Falcon Mk II photos flown 3 Oct 2018-19 Jun 2019), edited to bare ground, 5 m record with a 2 m ZIP (the record also says "GSD original de 10 m", unresolved), RMSEz 0.145 m (Madeira) and 0.051 m (Porto Santo), whole archipelago incl. Desertas and Selvagens; "Acesso e uso sem condições" (no named licence); direct ZIPs without login (MDT_2m.zip 460,132,910 bytes, mdt_5m.zip 114,126,877 bytes, by HEAD), files modified 2026-06-12. No Madeira lidar found (the national DGT lidar is mainland-only). https://snig.dgterritorio.gov.pt/rndg/srv/api/records/abba9a83-cb1c-4d2c-ad1d-ccb592247754/formatters/xml ; https://irig-storage.madeira.gov.pt/irig-catalogo/MDT_2m.zip (HEAD) ; https://irig-sig.madeira.gov.pt/ows/pg/qwc_config/DIR_modelo_digital_terreno?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetCapabilities ; https://irig-catalogo.madeira.gov.pt/geonetwork/srv/api/search/records/_search .

**Malta.** Planning Authority INSPIRE "Digital Terrain Model 2012" and "Digital Surface Model 2012": 1 m grids from the airborne lidar of the ERDF156 project, flown 17 Feb 2012 (35,081 x 28,238 cells, UTM 33N), WCS 2.0.1; data.europa.eu lists the distributions under CC BY 4.0 (the INSPIRE service records say noConditionsApply). A "2018" DTM/DSM is advertised on a 0.15 m grid but returns 8-bit values 101-108 over 10 m, i.e. not a usable DTM, and repeats the 2012 flight date. No open point cloud found; msdi.data.gov.mt and portal.data.gov.mt answered 403 (Cloudflare, not bypassed). https://malta.coverage.wetransform.eu/dtm_1m_2012/ows?SERVICE=WCS&VERSION=2.0.1&REQUEST=DescribeCoverage&COVERAGEID=dtm_1m_2012 ; https://data.europa.eu/api/hub/search/datasets/e2df40e5-6dff-4571-a9c9-dc45f3db11e2 ; https://data.europa.eu/api/hub/search/datasets/d8d5aca3-b980-4f42-914e-f35f2c6e3a3f ; https://data.europa.eu/api/hub/search/datasets/dcaf8e24-61ac-4b9e-9ec8-27263d9cb084 .

**Andorra.** The Govern d'Andorra (IDE Andorra) MDT: 15 m, 50 m and 100 m grids (keyword "2003"; record created 2013), "Propietat intel·lectual exclusiva del Govern d'Andorra. Prohibida la reproducció total o parcial" without express permission; download=false in the catalogue; the "Web de descàrregues" says download is "lliure" after entering personal data and accepting conditions (conditions not read, no download attempted). Catalogue searches for lidar and elevations return 0 records. Neighbours: Catalonia's ICGC lidar (at least 8 pts/m2) is Catalonia only; IGN LiDAR HD, my WFS tile-index check: 277 published tiles in the box 42.42-42.67 N, 1.41-1.79 E but 0 in the principality's centre (42.48-42.58 N, 1.40-1.70 E) and 1 at its southern edge, so the interior is not covered. Fallback: global products. NON-GOVERNMENTAL: University of Lausanne terrestrial lidar at 12 sites (research, not public); ARI BOSPIR-3D (2025-2028). https://www.ideandorra.ad/Cataleg/srv/cat/q?any=MDT&fast=index&to=50 ; https://www.ideandorra.ad/Cataleg/srv/cat/q?any=lidar&fast=index&to=50 ; https://www.cartografia.ad/web-de-descarregues ; https://www.cartografia.ad/serveis-ogc ; https://www.icgc.cat/en/Geoinformation-and-Maps/Data-and-products/Elevations/Territorial-elevations/Territorial-LiDAR ; https://wp.unil.ch/risk/andorra-la-vella-lidar-monitoring/ ; https://ari.ad/en/projects/bospir-3d ; https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=IGNF_LIDAR-HD_METADONNEE:metadata&resultType=hits&CQL_FILTER=BBOX%28geom%2C42.42%2C1.41%2C42.67%2C1.79%29 ; https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=IGNF_LIDAR-HD_METADONNEE:metadata&resultType=hits&CQL_FILTER=BBOX%28geom%2C42.48%2C1.40%2C42.58%2C1.70%29 ; https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=IGNF_LIDAR-HD_METADONNEE:metadata&resultType=hits&CQL_FILTER=BBOX%28geom%2C42.43%2C1.45%2C42.48%2C1.75%29 .

**Monaco.** No Monegasque elevation dataset found (the 2024 "3D capture campaign" for a digital twin has no open release; geoportail.gouv.mc redirects to a parcel viewer; data.gouv.mc answered 503). But IGN's LiDAR HD covers it: the IGN WFS tile index returns 20 published tiles in the box 43.72-43.76 N, 7.40-7.45 E (two sampled 1 km tiles: Riegl VQ-780 II-S, 62 and 65 M points, flights ending 2021-06-27, edition 2025-06-20, 0.5 m MNT tiles), Licence Ouverte 2.0. Fallback: global products. https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=IGNF_LIDAR-HD_METADONNEE:metadata&resultType=hits&CQL_FILTER=BBOX%28geom%2C43.72%2C7.40%2C43.76%2C7.45%29 ; https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=IGNF_LIDAR-HD_METADONNEE:metadata&outputFormat=application/json&count=2&CQL_FILTER=BBOX%28geom%2C43.72%2C7.40%2C43.76%2C7.45%29&propertyName=url_mnt,date_fin_acquisition,date_edition,nombre_points,capteur ; https://monentreprise.gouv.mc/en/news/the-principality-in-three-dimensions ; https://geoportail.gouv.mc/ .

## 4. Global and pan-European fallbacks; non-governmental sources [lead]

"Offline" = may I download and cache or tile it for my own offline map.

### 4.1 Global and pan-European DEMs (the fallback where nothing better is open)

| Product | What | Posting / accuracy | Licence (short) | Personal offline copy? | Access | URL |
|---|---|---|---|---|---|---|
| Copernicus DEM GLO-30 and GLO-90 (ESA/EU; made by Airbus) | Digital Surface Model (includes buildings, vegetation), TanDEM-X data 2011-2015, three instances EEA-10, GLO-30, GLO-90 | GLO-30 1", GLO-90 3"; absolute vertical accuracy below 4 m (90 %), relative below 2 m on slopes up to 20 %; heights EGM2008 | the CDSE page lets the "General public" download GLO-90 and GLO-30; the ESA licence PDF text says its general-public grant concerns the 90 m instance and that the 30 m and 10 m instances are under a separate licence, "distribution to the general public ... expressly excluded" from it: an ambiguity to check before shipping GLO-30 tiles; the AWS bucket serves "GLO-30 Public" anonymously | yes for private use of the public bucket; check the licence for redistribution | CDSE API/S3; anonymous AWS COGs | https://dataspace.copernicus.eu/explore-data/data-collections/copernicus-contributing-missions/collections-description/COP-DEM ; https://dataspace.copernicus.eu/sites/default/files/media/files/2025-06/copernicus_contributing_mission_data_access_v2_cop_dem_licenses.pdf ; https://copernicus-dem-30m.s3.amazonaws.com/readme.html |
| Copernicus EEA-10 ("GLO-10 for EEA countries") | 10 m instance (INSPIRE 0.3" / DGED 0.4") over the EEA-39 area | 10 m | downloadable only by registered public authorities, Copernicus services, EU institutions, EU-funded research projects and Copernicus operators; the general public may only view it | NO for a personal map | CDSE | same COP-DEM page as above |
| EU-DEM v1.1 | 25 m pan-European surface model; per OpenDEM "the only available pan-European DEM in EVRS" | 25 m | not read (the Copernicus Land page URL I tried returned 404) | unverified | n/a | https://opendem.info/opendemeu_background.html |
| FABDEM (University of Bristol; NON-GOVERNMENTAL) | derived from GLO-30 (description page timed out, unverified here) | 1" | the licence file at the Bristol record is the CC BY-NC-SA 4.0 text | private non-commercial yes; share-alike on derivatives | anonymous download | https://data.bris.ac.uk/datasets/s5hqmjcdj8yo2ibzi9b4ew3sn/license.txt |
| GEDTM30 (OpenGeoHub / Open-Earth-Monitor, Zenodo; NON-GOVERNMENTAL) | global ensemble 30 m bare-earth DTM, "Use for testing purposes only" (record disclaimer) | 30 m | CC BY 4.0 | yes | Zenodo | https://zenodo.org/api/records/18887460 ; https://zenodo.org/api/records?q=GEDTM30&size=3 |
| ALOS AW3D30 (JAXA) | global DSM ~30 m from PRISM | 1" | commercial and non-commercial use free under JAXA's terms | yes | JAXA download | https://www.eorc.jaxa.jp/ALOS/en/dataset/aw3d30/aw3d30_e.htm |
| MERIT DEM (U. Tokyo; NON-GOVERNMENTAL) | error-reduced global DEM | 3" | registration via a Google Form with a licence agreement | registration needed | form | https://global-hydrodynamics.github.io/MERIT_DEM/ |
| Mapzen/Tilezen terrain tiles on AWS | frozen global tiles (Joerd); sources under per-source attribution | coarse | attribution file linked | yes with credit | anonymous S3 | https://registry.opendata.aws/terrain-tiles/ |

For nearly every jurisdiction above with an open 0.5-1 m national or regional DTM, the national source beats these by a factor of 30 or more in cell size and removes the canopy and buildings that GLO-30 (a DSM) keeps. Jurisdictions where only the global products remain open (nothing finer is open): Jersey, Guernsey, Gibraltar, San Marino, Andorra (its 15 m MDT is copyright-restricted) and the Isle of Man (finer data is sold). Inside other jurisdictions the global products fill the gaps between partial open footprints: Scotland (outside the Land LiDAR Programme releases), Northern Ireland, Ireland, and Italian areas outside the PST-A corridors, where TINITALY 10 m or the regional 5 m grids come first.

### 4.2 Non-governmental sources (all NON-GOVERNMENTAL)

| Source | Type | What / resolution / Western-Southern Europe area | Beats national or global? | Licence (quote) | Offline cache | Access | URL |
|---|---|---|---|---|---|---|---|
| Mapterhorn | open-source project funded by NLnet NGI0; maintainers named on its site; repackages government data | Terrarium-encoded WebP terrain tiles (512 px) as PMTiles, z0-z17; 151 source datasets. European sources listed on its attribution page: Austria (BEV 1 m, Burgenland 5 m, Kärnten, Oberösterreich, Salzburg 1 m), Belgium (Flanders DTM 1 m, Wallonia MNT 2021-22 at 0.5 m), France (IGN LiDAR HD MNT, RGE ALTI 1 m and 5 m including Corsica and the DROM), Germany (all 16 Länder; BW 0.25 m, others 1 m), Italy (TINITALY 10 m, Valle d'Aosta, Bolzano 2.5 m, Lombardia 5 m, Piemonte 5 m, Sardegna coast and towns 1 m, Sicilia 2 m, Trento), Luxembourg (LiDAR 2019 MNT), Netherlands (AHN5 5 m, Caribbean 0.5 m), Portugal (DGT, Madeira), Spain (MDT02, MDT05, MDT50), Switzerland (swissALTI3D, Zürich), UK (England 1 m, Scotland Phase 1, Wales 1 m); GLO-30 elsewhere. Not in the list: Ireland, Northern Ireland, Catalunya's ICGC, Navarra, Euskadi, the Azores, Malta, Andorra, Monaco, San Marino | no (a re-packager at the same or coarser resolution; it is the practical channel) | code BSD-3; terrain data per source: CC BY 4.0, CC0, dl-de/zero, dl-de/by-2-0, Licence Ouverte 2.0, OGL, "Model license for free reuse", CC BY 2.5 (Trento) | YES: area extracts with the `pmtiles` CLI from download.mapterhorn.com; attribution per source is required | anonymous HTTPS PMTiles + z/x/y endpoint | https://mapterhorn.com/ ; https://mapterhorn.com/data-access ; https://mapterhorn.com/coverage ; https://download.mapterhorn.com/attribution.json |
| Sonny's LiDAR Digital Terrain Models of Europe | individual compiler (Austrian enthusiast), "compiled and resampled by Sonny" from the best open sources | DTMs at 0.5" (Austria, Switzerland only), 1" (about 20 x 30 m) and 3" in HGT, 20 m and 50 m GeoTIFFs; he declines to offer finer grids; version history shows Germany v3b (2024-09-08), UK v2 and Ireland v1 (2023), Italy v2b (2024), Portugal mainland v1 (2025-08-08), Austria and Switzerland 0.5" v2 (2024-04), "EUROPE v25" (2025-08-08) | no (coarser than the national 1 m sources; useful as a ready one-click compilation) | CC BY 4.0, attribution to Sonny and a link to his site | YES (CC BY 4.0) | Google Drive downloads (the "Download all" button for logged-out users was removed in 2026) | https://sonny.4lima.de/ |
| OpenDEM (OpenDemEurope, OpenDTM-DE, OpenDemSearcher) | open project/portal | pan-European DTM in 50 x 50 km tiles, EPSG:3035, vertical EVRS2000, resolutions below 1 m recomputed to 1 m; 1-arc-second tiles; OpenDTM-DE for Germany; a searcher listing free DEMs (for example Portugal 0.5 m DTM/DSM: "Registration required") | no (re-processed national data) | "multiple licenses depending on the region" (schema.org record on the download page) | per source licence | web download | https://opendem.info/ ; https://opendem.info/opendemeu_download_highres.html ; https://opendem.info/opendemeu_background.html |
| OpenTopography (SDSC, UC San Diego) | academic facility | DCAT catalogue of 811 datasets; in my box of the catalogue (-32 to 22 E, 32 to 62 N) 33 entries: mostly small research patches (Stromboli SfM surveys 2018-2025, fault trenches in Italy and Slovenia, a Skye survey, Selmun promontory Malta, Lauterbrunnen rockwalls), plus the whole-country AHN4 (Netherlands, CC0, modified 2026-01-15) and "Switzerland 50 Centimeter DEMs" (swisstopo terms, 2026-07-18) | only inside patches; the two national sets are copies | per dataset: CC BY 4.0, CC0, swisstopo terms; several blank | yes for CC BY/CC0 | OpenTopography download pages | https://portal.opentopography.org/ ; https://portal.opentopography.org/csw/catalog.json |
| UKCEH "Merged LiDAR DTM and DSM for England and Wales" | public research body (NERC) | 1 m, merged from EA 2023 and NRW 2024, 615 GB, published 2026-08-07 | no | OGL | yes | EIDC | https://catalogue.ceh.ac.uk/documents/34443359-64c0-4909-9f27-bb5d47f1153f?format=json |
| TerraLab "LiDAR HD par commune" | NGO/individual | CSV/GPKG of IGN LiDAR HD flight date and density per commune | not terrain | Licence Ouverte | yes | data.gouv.fr | https://www.data.gouv.fr/api/1/datasets/?q=lidar&page_size=50&page=2 |
| Viewfinder Panoramas (J. de Ferranti) | individual | global 3" and 15" tiles; 1" DEMs for Europe (updates listed, including Sierra de Gredos and Scandinavia); built from public sources | no | licence line not read | not verified | zip downloads | https://www.viewfinderpanoramas.org/dem3.html |
| Airbus WorldDEM Neo | commercial | 5 m DSM and DTM worldwide, data 2017-2021; UP42 price 750 credits/km2 (DSM), 964 (DTM); WGS84 with EGM2008 heights, horizontal 6 m CE90 | yes (5 m vs 30 m) but not a lidar-class model | Airbus/UP42 licence not read | NO unless licensed | UP42 catalogue | https://docs.up42.com/data/worlddem-neo |
| Intermap NEXTMap | commercial | spaceborne 1/6/10 m and airborne 1/5 m DSM+DTM; vertical accuracy "up to 1 m (depending on area)" | possibly, paid | not read | NO unless licensed | sales | https://www.intermap.com/nextmap |
| Vantor (ex-Maxar) Precision3D | commercial | DSM at 1 m and derived terrain products from satellite stereo, listed by NASA's CSDA programme | possibly, paid | not read | NO unless licensed | commercial order | https://www.earthdata.nasa.gov/data/alerts-outages/digital-surface-terrain-models-from-vantors-precision3d-product-line-added |
| Bluesky International | commercial UK aerial survey firm | DTM and DSM from its own lidar (page: "LiDAR > Our LiDAR data is captured ... to create DTMs and DSMs"); flew Isle of Man 2013, Scotland and Northern Ireland coastal programmes for governments | yes where sold (Isle of Man, Channel Islands) | commercial | NO unless licensed | purchase | https://www.bluesky-world.com/ |
| Getmapping | commercial UK aerial survey firm | home page fetched; no elevation product text that I could read | n/a | n/a | n/a | n/a | https://www.getmapping.com/ |
| Cesium World Terrain | commercial platform | composite whose attribution list names EU-DEM, the Environment Agency 1 m and 2 m composite DTM and USGS data | no evidence of better than the open sources | caching terms not read | assume NO | Cesium ion | https://cesium.com/platform/cesium-ion/content/cesium-world-terrain/ |
| MapTiler Terrain / Mapbox Terrain-DEM | commercial tile services | MapTiler terms: "prohibited to batch or excessive bulk download of map tiles" and to export map content; Mapbox: Terrain-RGB v1 tileset for its GL JS and mobile SDKs (licence terms not read) | no | MapTiler as quoted; Mapbox unread | NO (MapTiler); unverified (Mapbox) | API key | https://www.maptiler.com/cloud/terms/ ; https://docs.mapbox.com/data/tilesets/reference/mapbox-terrain-dem-v1/ |
| East View Geospatial; Digimap (Guernsey/Jersey) | commercial map sellers | contour-based maps of Gibraltar; community maps for Guernsey | no | commercial | NO | purchase | https://geospatial.com/country_profiles/gibraltar/ ; https://www.digimap.gg/ |

Bottom line: no non-governmental source beats the national or regional data above. The useful ones are the channels that already carry it in an offline-friendly format (Mapterhorn PMTiles under per-source open licences; Sonny's DTMs at CC BY 4.0, but coarser) and research patches on OpenTopography. Commercial DEMs are paid and none was found that permits an offline cache for a personal map without a negotiated licence.

## 5. Unverified (each with every route tried)

Items are listed only where every route failed; partial answers are in the notes above.

- Kärnten's 2022-2029 laser products (grid, openness, files): gis.ktn.gv.at HEAD, GET and ranged GET (4 tries, 60 s timeouts); data.gv.at hub metadata (lists the Raster and Punktwolke URLs, no resolution); web search found nothing.
- Vorarlberg 2023 50 cm file download: data.gv.at distributions and ISO record links led only to a WMS.
- Steiermark: whether the second-campaign 50 cm products (2023-26) are online: the portal is a JavaScript page; the landesentwicklung pages give the campaign but not the download state.
- Isle of Man: whether an open DTM exists today: gov.im pages (business-customers mapping, open data) answered "Request Rejected" or an empty body (WAF) on three runs, a Wayback copy of the mapping page came back empty (HTTP 200, 0 bytes), data.gov.im did not answer; the only finding is the 2013 lidar resold by Bluesky (2016 article).
- Gibraltar: any elevation grid on geoportal.gov.gi (403), data.gov.gi (no DNS), gibraltar.gov.gi/open-data (404), a landinfo.com PDF (403); only the 2019 and 2023 press releases were read.
- Guernsey, Alderney, Sark, Herm: no States of Guernsey GIS or open-data portal located (digimap.gg read; searches).
- Jersey: whether Jersey Heritage published its 2020 lidar (news article only; opendata.gov.je CKAN searches return nothing).
- Portugal: Lisbon, Porto, Setúbal municipal lidar (dados.cm-lisboa.pt Cloudflare 403; dadosabertos.cm-lisboa.pt NXDOMAIN; no Porto or Setúbal data found); the mainland vertical datum (not in the SNIG record or the STAC item); the DGT free-account sign-up (not attempted by rule); Azores vertical datums for island groups other than the central one; the Madeira lidar-systems tender (TED PDF not text-readable); the record's own 5 m / 10 m GSD inconsistency.
- Malta: real vertical accuracy; a newer lidar than 2012 (the "2018" WCS is not usable); the Planning Authority FTP/laz-on-request route (msdi.data.gov.mt and portal.data.gov.mt: Cloudflare 403; Wayback 404).
- Andorra: the conditions shown by the download app and whether it holds anything finer than 15 m (JavaScript app; conditions not read, nothing downloaded).
- Monaco: any Monegasque lidar release (data.gouv.mc answered 503 twice; geoportail.gouv.mc redirects to a parcel viewer). IGN LiDAR HD coverage itself was verified by the WFS index (section 3.8).
- San Marino: any geoportal or DTM of the Republic (gov.sm UTCC page read; mappeonline.pa.sm not fetched); whether Emilia-Romagna's or PST-A tiles cover its territory.
- Vatican City: Vatican-published releases (no official site fetched); its boundary was not fetched.
- France: vertical accuracy of IGN LiDAR HD (specification PDF without extractable text; geoservices PDFs redirect); IGN's progress map (login wall); PACA/Sud (DataSud 403), Normandie (HTML only), Centre-Val de Loire and Bourgogne-Franche-Comté (0 hits or 411), Île-de-France (0 hits) regional catalogues; several regional hosts did not resolve.
- Belgium/Netherlands/Luxembourg: terms of Flanders's raw point cloud (two pages disagree); Brussels vertical accuracy; AHN5 acquisition years and density, AHN5/6 accuracy; Luxembourg 2024 accuracy (13 MB documentation PDF over the limit).
- Germany: BKG DGM1 vertical accuracy; BKG DGM50/DGM10 beyond "not in the current catalogue"; BW portal behaviour (Angular app) and ALS_3 share; Hamburg's official point-cloud density and offer; Hessen Laserscan 3 progress and cloud price; Mecklenburg-Vorpommern DGM1 accuracy and flight years (image-only PDFs); Saarland accuracy, density, flight dates and any point cloud (official LVGL pages 403 behind a bot shield; Wayback 403); Schleswig-Holstein flight years and point cloud (service.gdi-sh.de timed out three times; the download host fails TLS verification, not bypassed); Thüringen coverage share of 2020-2025 (live geoportal outage, CAPTCHAs; Wayback and CSW used); Rheinland-Pfalz geoshop cart flow; Bremen's point-cloud price; coastal and federal-water products (not researched).
- Switzerland/Liechtenstein: the download route of Geneva's 2025 point cloud (CKAN record lists no resources); Basel-Landschaft's usage terms; Lausanne and Riviera lidar details; Liechtenstein's own geoportal (TLS error twice, 403).
- Italy: whether any 25 cm tile of the new national lidar is public; first-hand behaviour of the PST-A web app (JavaScript shell); live endpoints of Piemonte, Trento (siat.provincia.tn.it), Umbria (all hosts timed out), Marche (CAPTCHA, Wayback used), Calabria, Basilicata (RSDI timed out); Abruzzo's regional DTM details; Puglia's SIT download (login gateway) and IODL 2.0 text; Sardegna and Toscana download mechanics; Bolzano's point-cloud download path and terms; accuracy of Valle d'Aosta's 2005/2008 flights; Lazio and Sicilia licence coverage of the ImageServer.
- Spain: third-coverage vertical accuracy on a fetched page (none found; the specification page states RMSE z at most 10 cm for the programme); Asturias regional licence and any regional 1 m product (ideas.asturias.es timed out 3 times, Wayback copy is an empty shell); Cantabria download centre and licence; Bizkaia and Araba own products (JavaScript catalogues, opendatabizkaia.eus timed out 3 times); Ceuta and Melilla government IDEs (hosts did not resolve); La Rioja live listings (Cloudflare 403/522); Murcia IDE (CAPTCHA, stopped); ITACyL raw lidar 2010 (search snippet only); Aragón's lidar viewer (timed out); Galicia's live IET viewer (timed out; Wayback read); IDEIB raw-grid download.
- Pan-European: EU-DEM v1.1 licence and producer page (land.copernicus.eu URL 404; spacedata.copernicus.eu timed out); FABDEM description (the Bristol dataset page timed out twice); Mapbox, Cesium, Esri, Planet licence terms (not read); the exact applicable licence for redistributing GLO-30 tiles (section 4.1).

## 6. Conduct notes

- All requests were made with User-Agent `scenic-roads/0.1 (personal offline map)` through two logging shell helpers; one request at a time per host with a 1 s pause. Fetch logs of every agent and of mine are kept in the session scratchpad: eu-logs/ (uk.log, fr.log, de1.log, de2.log, chat.log, it1.log, it2.log, es1.log, es2.log, pt.log, lead.log, lead2.log, lead3.log; about 1,900 lines). One it1 request was made with a direct `curl -I` and logged by hand.
- No data was downloaded. Largest bodies read: index and listing pages up to about 4.4 MB (Brandenburg ALS tile listing, NRW XML indexes 3.3 and 3.5 MB, Trento open-directory listing 3.1 MB, Navarra LAZ listing 2.1 MB, data.public.lu lidar search 3.6 MB, OpenTopography catalogue 1.4 MB), PDFs up to 3.2 MB; everything else was a HEAD or a small range read. The 5.39 MB IGN WFS capabilities answer and the IGN specification PDF were not read whole. A few GetCoverage/GetMap probes returned under 7 KB or 0.5 MB.
- Blocks respected and not bypassed: gov.im (WAF), gov.ie press release (403), geoportal.gov.gi (403), msdi/portal.data.gov.mt and dados.cm-lisboa.pt (Cloudflare 403), La Rioja hosts (Cloudflare 403, 522), sitmurcia.carm.es and regione.marche.it (CAPTCHA), saarland.de (bot shield), geoportal.thueringen.de and tlbg.thueringen.de (CAPTCHA), metaver.de (429; waited, then Wayback), idecyl and gdi.berlin.de JSON POST searches (403; GET used), logins: DGT CDD download (Keycloak login, stopped), SIT Puglia (login gateway, stopped), NÖ Geoshop (login, stopped). The CNIG download form's anti-automation script was read in the page source and not exercised. Wayback copies were used for pages that timed out or were blocked (named in the notes).
- Several hosts did not resolve or timed out for long periods from here (inspire-geoportal.ec.europa.eu, many Italian and Spanish regional hosts); the Italian national catalogue RNDT and datos.gob.es served as substitutes.
- A background fetch loop and a stuck EA OGC API query were stopped by their exact task ids (uk and chat agents); no process was left running. Web search was used only to discover URLs.
- The sandbox clock reads 2026-11-13 UTC for log timestamps (evening of 2026-11-12 local).
- The sub-agents were refused when they tried to write FINDINGS.md files into their scratch folders (the Write tool blocks report files); they returned their findings as text, which I merged. No file other than this one was written in the repository; git was not touched.

## 7. URLs read

Every URL cited in this file, each fetched in this run with a 2xx answer (HEAD for those marked HEAD in the notes). The complete fetch logs are named in section 6.

- http://geoportale.regione.abruzzo.it/Cartanet/
- http://metadati.partout.it/metadata_documents/CC_BY_DTM_v2.pdf
- http://tinitaly.pi.ingv.it/TINItaly_1_1/wcs?service=WCS&request=getCapabilities
- http://wms.pcn.minambiente.it/ogc?map=/ms_ogc/WMS_v1.3/Vettoriali/QU_Lidar.map&service=WMS&request=GetCapabilities
- http://wms.pcn.minambiente.it/ogc?map=/ms_ogc/WMS_v1.3/servizi-LiDAR/LIDAR_LIGURIA.map&service=wms&request=getCapabilities&version=1.3.0
- http://wms.pcn.minambiente.it/ogc?map=/ms_ogc/wfs/QU_Lidar.map&service=WFS&version=1.1.0&request=GetFeature&typeName=EL.QUADRO.UNIONE.LIDAR.0101&PROPERTYNAME=regione,contratto,data,tavola&BBOX=41.88,12.43,41.92,12.47,urn:ogc:def:crs:EPSG::4326
- https://admin.opendatani.gov.uk/api/3/action/package_search?q=lidar&rows=30
- https://admin.opendatani.gov.uk/api/3/action/package_show?id=osni-open-data-10m-dtm-sheets-1-50
- https://api.os.uk/downloads/v1/products
- https://api.os.uk/downloads/v1/products/Terrain50
- https://api.os.uk/downloads/v1/products/Terrain50/downloads
- https://archiv.transparenz.hamburg.de/hmbtgarchive/HMDK/dgm1_hh_2022-04-30_258334_snap_1.zip
- https://ari.ad/en/projects/bospir-3d
- https://b5m.gipuzkoa.eus/lidar/
- https://b5m.gipuzkoa.eus/web5000/en/legal-information
- https://blogs.gov.scot/digital/2026/01/29/scotlands-lidar-revolution-first-data-release-to-reveal-scotlands-landscape-in-unprecedented-detail/
- https://blogs.gov.scot/digital/2026/10/12/scottish-lidar-user-conference-and-new-land-and-coastal-lidar-data-released/
- https://cartes.gouv.fr/cgu
- https://cartografia.castillalamancha.es/descargas-datos%20geograficos
- https://catalegs.ide.cat/geonetwork/srv/api/records/lidar-territorial-v3r1-2021-2023
- https://catalogue.ceh.ac.uk/documents/34443359-64c0-4909-9f27-bb5d47f1153f?format=json
- https://cdd.dgterritorio.gov.pt/dgt-be/v1/collections
- https://cdd.dgterritorio.gov.pt/dgt-be/v1/collections/ACORES-MDE-1/items/2024_SMG_PDL_31-2-3-4_01_MDE
- https://cdd.dgterritorio.gov.pt/dgt-be/v1/collections/MDT-50cm/items/MDT-50cm-205466-07-2025
- https://centrodedescargas.cnig.es/CentroDescargas/faqs
- https://centrodedescargas.cnig.es/CentroDescargas/lidar-segunda-cobertura
- https://centrodedescargas.cnig.es/CentroDescargas/lidar-tercera-cobertura
- https://centrodedescargas.cnig.es/CentroDescargas/modelo-digital-superficies-mds50cm-tercera-cobertura
- https://centrodedescargas.cnig.es/CentroDescargas/modelo-digital-terreno-mdt02-segunda-cobertura
- https://centrodedescargas.cnig.es/CentroDescargas/modelo-digital-terreno-mdt50cm
- https://centrodedescargas.cnig.es/CentroDescargas/modelos-digitales-elevaciones
- https://centrodedescargas.cnig.es/CentroDescargas/politica-datos
- https://cesium.com/platform/cesium-ion/content/cesium-world-terrain/
- https://ckan.opendata.swiss/api/3/action/package_search?fq=organization:amt-fuer-tiefbau-und-geoinformation-fl&rows=100&fl=name,title,metadata_modified
- https://cms.lgln.niedersachsen.de/fb24/aktualitaet_als.pdf
- https://cms.lgln.niedersachsen.de/fb24/dgm1_dom1.pdf
- https://copernicus-dem-30m.s3.amazonaws.com/readme.html
- https://dadesobertes.gva.es/api/3/action/package_search?q=MDT&rows=100
- https://dadesobertes.gva.es/api/3/action/package_search?q=lidar&rows=100&fl=name,title
- https://dadesobertes.gva.es/api/3/action/package_show?id=lidar-2017-de-1-punto-por-metro-cuadrado-de-la-provincia-de-castellon
- https://data.bev.gv.at/download/ALS/DTM/20250915/ALS_DTM_CRS3035RES50000mN2650000E4650000.tif
- https://data.bev.gv.at/geonetwork/srv/api/search/records/_search
- https://data.bris.ac.uk/datasets/s5hqmjcdj8yo2ibzi9b4ew3sn/license.txt
- https://data.europa.eu/api/hub/search/datasets/d8d5aca3-b980-4f42-914e-f35f2c6e3a3f
- https://data.europa.eu/api/hub/search/datasets/dcaf8e24-61ac-4b9e-9ec8-27263d9cb084
- https://data.europa.eu/api/hub/search/datasets/e2df40e5-6dff-4571-a9c9-dc45f3db11e2
- https://data.geo.admin.ch/api/stac/v0.9/collections/ch.swisstopo.swissalti3d
- https://data.geo.admin.ch/api/stac/v0.9/collections/ch.swisstopo.swissalti3d/items?datetime=2026-01-01T00:00:00Z/..&limit=1
- https://data.geobasis-bb.de/geobasis/daten/als/laz/
- https://data.geobasis-bb.de/geobasis/daten/dgm/
- https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=DescribeFeatureType&TYPENAME=IGNF_LIDAR-HD_METADONNEE:metadata
- https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=IGNF_LIDAR-HD_METADONNEE:metadata&outputFormat=application/json&count=2&CQL_FILTER=BBOX%28geom%2C43.72%2C7.40%2C43.76%2C7.45%29&propertyName=url_mnt,date_fin_acquisition,date_edition,nombre_points,capteur
- https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=IGNF_LIDAR-HD_METADONNEE:metadata&resultType=hits
- https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=IGNF_LIDAR-HD_METADONNEE:metadata&resultType=hits&CQL_FILTER=BBOX%28geom%2C41.3%2C8.4%2C43.1%2C9.7%29
- https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=IGNF_LIDAR-HD_METADONNEE:metadata&resultType=hits&CQL_FILTER=BBOX%28geom%2C42.42%2C1.41%2C42.67%2C1.79%29
- https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=IGNF_LIDAR-HD_METADONNEE:metadata&resultType=hits&CQL_FILTER=BBOX%28geom%2C42.43%2C1.45%2C42.48%2C1.75%29
- https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=IGNF_LIDAR-HD_METADONNEE:metadata&resultType=hits&CQL_FILTER=BBOX%28geom%2C42.48%2C1.40%2C42.58%2C1.70%29
- https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=IGNF_LIDAR-HD_METADONNEE:metadata&resultType=hits&CQL_FILTER=BBOX%28geom%2C43.72%2C7.40%2C43.76%2C7.45%29
- https://data.geopf.fr/wms-r?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetCapabilities
- https://data.gov.ie/api/3/action/package_search?q=lidar&rows=40
- https://data.overheid.nl/en/dataset/47567-actueel-hoogtebestand-nederland--ahn--dtm
- https://data.public.lu/api/1/datasets/?q=lidar&page_size=20
- https://data.public.lu/api/1/datasets/lidar-2024-releve-3d-du-territoire-luxembourgeois/
- https://data.strasbourg.eu/api/explore/v2.1/catalog/datasets/nuage_de_points_3d_2025
- https://data.strasbourg.eu/api/explore/v2.1/catalog/datasets/raster_elevation_sol_2025
- https://datacloud.icgc.cat/datacloud/lidar-territorial/
- https://datacloud.icgc.cat/datacloud/model-elevacions-terreny/tif_unzip/
- https://datamap.gov.wales/layers/geonode:nrw_lidar_tile_catalogue_archive
- https://datamap.gov.wales/layers/geonode:welsh_government_lidar_tile_catalogue_2020_2023
- https://datamap.gov.wales/maps/6969
- https://dataspace.copernicus.eu/explore-data/data-collections/copernicus-contributing-missions/collections-description/COP-DEM
- https://dataspace.copernicus.eu/sites/default/files/media/files/2025-06/copernicus_contributing_mission_data_access_v2_cop_dem_licenses.pdf
- https://daten.geo.lu.ch/produkt/meta/DTM24025_DS
- https://dati.toscana.it/api/3/action/package_show?id=dtm-2017
- https://datos.canarias.es/portal/aviso-legal-y-condiciones-de-uso
- https://datos.gob.es/apidata/catalog/dataset/a03002951-elevaciones-asturias.json
- https://datos.gob.es/apidata/catalog/dataset/a15002917-modelo-digital-del-terreno-mdt-procedente-de-lidar-resolucion-2-metros-ano-2024.json
- https://datos.gob.es/apidata/catalog/dataset/e00125901-spaignmdt.json
- https://datos.gob.es/apidata/catalog/dataset/e0dat0002-modelo-digital-del-terreno-mdt-de-1m-de-la-comunidad-autonoma-del-pais-vasco-ano-2017.json
- https://datos.madrid.es/pages/condiciones-generales-ayuntamiento-de-madrid
- https://datosabiertos.regiondemurcia.es/api/3/action/package_search?q=lidar&rows=30
- https://dgm.stac.lgln.niedersachsen.de/collections
- https://dgm.stac.lgln.niedersachsen.de/collections/dgm1/items?limit=1&sortby=-datetime
- https://dgt-be.a.incd.pt:8081/collections/ACORES-MDE-1/items?limit=1
- https://dgt-be.a.incd.pt:8081/collections/MDT-50cm/items?limit=2
- https://dmwproductionblob.blob.core.windows.net/cogs/lidar/wales_dtm_16bit_cog.tif
- https://docs.mapbox.com/data/tilesets/reference/mapbox-terrain-dem-v1/
- https://docs.os.uk/os-downloads/products/land-and-terrain-portfolio/os-terrain-5/os-terrain-5-overview
- https://docs.os.uk/os-downloads/products/land-and-terrain-portfolio/os-terrain-5/os-terrain-5-overview/product-supply.md
- https://docs.os.uk/os-downloads/products/land-and-terrain-portfolio/os-terrain-50/os-terrain-50-overview
- https://docs.up42.com/data/worlddem-neo
- https://doris-metadaten.ooe.gv.at/at.gv.ooe.doris.geonetwork_extern/srv/api/records/68CFE491-7CDF-4781-A1F1-603CD0738ABD/formatters/xml
- https://download.data.public.lu/resources/bd-l-lidar2024-releve-3d-du-territoire-luxembourgeois/20241223-093912/MNT_Lidar2024.tif
- https://download.mapterhorn.com/attribution.json
- https://e-gov.ooe.gv.at/at.gv.ooe.dorisdaten/DORIS_Basisdaten/HOEHENMODELL_AKTUELLE_FLUGJAHRE.zip
- https://e-gov.ooe.gv.at/at.gv.ooe.intramapgem/dop/downloads/41501/41501_DGM_tif.zip
- https://eaglefvg.regione.fvg.it/eagle/main.aspx?configuration=guest
- https://environment.data.gov.uk/dataset/9ba4d5ac-d596-445a-9056-dae3ddec0178
- https://environment.data.gov.uk/spatialdata/lidar-composite-digital-terrain-model-dtm-1m/wcs?request=GetCapabilities&service=WCS&version=2.0.1
- https://environment.data.gov.uk/spatialdata/survey-index-files/ogc/features/v1/collections?f=json
- https://environment.data.gov.uk/support/faqs/275879146
- https://fbinter.stadt-berlin.de/fb_daten/beschreibung/dgm1.html
- https://filescartografia.navarra.es/5_LIDAR/_leeme.txt
- https://filescartografia.navarra.es/6_MDE/
- https://filescartografia.navarra.es/6_MDE/_leeme.txt
- https://fr.wikipedia.org/wiki/France_m%C3%A9tropolitaine
- https://gdi.berlin.de/data/a_als/atom/
- https://gdi.berlin.de/data/dgm1/atom/
- https://gdi.berlin.de/geonetwork/srv/api/records/85a97801-36bb-4627-8790-f5e53b1e38b3
- https://gdi.berlin.de/geonetwork/srv/api/records/f0e8ff09-2887-3446-9c53-81dbc45af03c
- https://gdz.bkg.bund.de/index.php/default/digitale-geodaten/digitale-gelandemodelle.html
- https://gdz.bkg.bund.de/index.php/default/digitale-geodaten/digitale-gelandemodelle/digitales-gelandemodell-gitterweite-1-m-dgm1.html
- https://gdz.bkg.bund.de/index.php/default/digitale-geodaten/digitale-gelandemodelle/digitales-gelandemodell-gitterweite-200-m-dgm200.html
- https://gdz.bkg.bund.de/index.php/default/digitale-geodaten/digitale-gelandemodelle/digitales-gelandemodell-gitterweite-25-m-dgm25.html
- https://gdz.bkg.bund.de/index.php/default/digitale-geodaten/digitale-gelandemodelle/digitales-gelandemodell-gitterweite-5-m-dgm5.html
- https://gdz.bkg.bund.de/index.php/default/digitale-geodaten/digitale-gelandemodelle/digitales-oberfaechenmodell-dom1.html
- https://gdz.bkg.bund.de/index.php/default/open-data.html
- https://gee-community-catalog.org/projects/tinitaly/
- https://geo.araba.eus/es/web/geo/centro-de-descargas
- https://geobasis-bb.de/lgb/de/geodaten/3d-produkte/gelaendemodell/
- https://geobasis-bb.de/lgb/de/geodaten/3d-produkte/laserscandaten/
- https://geobasis-bb.de/lgb/de/geodaten/open-data/
- https://geobroker.geobasis-bb.de/gbss.php?MODE=GetProductInformation&PRODUCTID=518094b9-d294-4e99-aeb2-550592a00682
- https://geobroker.geobasis-bb.de/gbss.php?MODE=GetProductInformation&PRODUCTID=d9895ec2-7039-4c0d-914c-a68f227a7069
- https://geobru-geonetwork.irisnet.be/geonetwork/srv/api/records/1d7bd49d-fe83-4388-af85-6f5dc8ec7909
- https://geobru-geonetwork.irisnet.be/geonetwork/srv/api/records/ff1124e1-424e-11ee-b156-00090ffe0001
- https://geodaten.bayern.de/odd/a/dgm/dgm1/meta/metalink/09.meta4
- https://geodaten.bayern.de/opengeodata/json/opengeodata_produkte.json
- https://geodaten.bgld.gv.at/de/downloads/hoehenmodelle-orthofotos.html
- https://geodati.gov.it/resource/id/adbpo:0abd7059-6d4b-48a3-8da8-18fe41
- https://geodati.gov.it/resource/id/p_TN:79c6cdf0-c73a-46ba-870c-704bf4e27ac3
- https://geodati.gov.it/resource/id/p_bz:Elevation:DigitalTerrainModel-0.5m
- https://geodati.gov.it/resource/id/p_bz:Elevation:DigitalTerrainModel-2.5m
- https://geodati.gov.it/resource/id/r_emiro:2026-09-03T160720
- https://geodati.gov.it/resource/id/r_friuve:m10856-cc-i11647
- https://geodati.gov.it/resource/id/r_friuve:m10856-cc-i11826
- https://geodati.gov.it/resource/id/r_liguri:D.2056
- https://geodati.gov.it/resource/id/r_lombar:975979a7-68e8-4524-b759-0db4d8284939
- https://geodati.gov.it/resource/id/r_veneto:c0101071_Lidar5m
- https://geomaps.regione.campania.it/server/rest/services/Amministrativo/CTR_DTM5m/ImageServer?f=json
- https://geonetwork1.civis.bz.it/geonetwork/srv/api/records/p_bz:Elevation:DigitalTerrainModel-EtschAdige-0.2m-2024
- https://geoportail.gouv.mc/
- https://geoportal.saarland.de/app-article/geobasisdatenuebersicht/
- https://geoportal.saarland.de/mapbender/php/mod_dataISOMetadata.php?outputFormat=iso19139&id=d73285be-e3a1-46ef-9562-88883dc0f9c0
- https://geoportale.regione.emilia-romagna.it/approfondimenti/prodotti-lidar-e-ortofoto
- https://geoportale.regione.emilia-romagna.it/approfondimenti/rer23_24
- https://geoportale.regione.lazio.it/api/v2/resources?search=dtm&page_size=30
- https://geoportale.regione.vda.it/download/dtm/
- https://geoservices9.civis.bz.it/geoserver/ows?service=WCS&version=2.0.1&request=GetCapabilities
- https://geoservicios.cantabria.es/inspire/rest/services/MDT/MapServer/layers?f=pjson
- https://geoservicios.cantabria.es/inspire/rest/services/MDT/MapServer?f=pjson
- https://geoshop.noel.gv.at/
- https://geoshop.rlp.de/opendata-dgm1.html
- https://geospatial.com/country_profiles/gibraltar/
- https://global-hydrodynamics.github.io/MERIT_DEM/
- https://gn.mase.gov.it/portale/distribuzione-dati-pst
- https://gn.mase.gov.it/portale/piano-straordinario-di-telerilevamento
- https://gn.mase.gov.it/portale/pst-dati-lidar
- https://gn.mase.gov.it/portale/wcs
- https://gsi.geodata.gov.ie/imagehost/rest/services/Lidar?f=json
- https://gsi.geodata.gov.ie/server/rest/services/Lidar?f=json
- https://hvbg.hessen.de/geoinformation/open-data
- https://hvbg.hessen.de/landesvermessung/geotopographie/3d-daten/airborne-laserscanning
- https://hvbg.hessen.de/landesvermessung/geotopographie/3d-daten/digitale-gelaendemodelle
- https://hvbg.hessen.de/sites/hvbg.hessen.de/files/2024-06/laserscan_3._befliegungsuebersicht-bf.pdf
- https://icearagon.aragon.es/descargas.jsp
- https://icearagon.aragon.es/geonetwork/srv/spa/q?any=lidar&fast=index&_content_type=json&resultType=details&to=30
- https://idecyl.jcyl.es/geonetwork/srv/api/records/SPAGOBCYLCITDTSELMDT0811
- https://idee.es/csw-inspire-idee/static/api/records/spaignMDT02
- https://ideg.xunta.gal/servizos/rest/services/Elevacions/Mdt2_PNOA09_10/ImageServer?f=pjson
- https://ideib.caib.es/geoserveis/rest/services/public/2021BTIB5000/MapServer/32/iteminfo?f=pjson
- https://ideib.caib.es/geoserveis/rest/services/public/GOIB_Altimetria_IB/MapServer/layers?f=pjson
- https://idem.comunidad.madrid/catalogocartografia/srv/spa/q?any=lidar&_content_type=json&fast=index&resultType=details&to=40
- https://idem.comunidad.madrid/catalogocartografia/srv/spa/q?any=terreno&_content_type=json&fast=index&resultType=details&to=40
- https://idena.navarra.es/ogc/wcs?service=WCS&request=GetCapabilities
- https://iderioja.github.io/doc_lidar_2016_processing_en/dtm_dsm/digital_terrain_models/
- https://iderioja.github.io/doc_lidar_2016_processing_en/license/
- https://iderioja.github.io/doc_lidar_2016_processing_en/processing/introduction/
- https://idt2.regione.veneto.it/idt/downloader/download
- https://idt2.regione.veneto.it/nuovo-dtm-derivato-da-dati-lidar/
- https://ie.openprocurements.com/tender/lidar-and-orthoimagery-services-for-national-parks-and-wildlife-service-npws-department-of-housing-l/
- https://irig-catalogo.madeira.gov.pt/geonetwork/srv/api/search/records/_search
- https://irig-sig.madeira.gov.pt/ows/pg/qwc_config/DIR_modelo_digital_terreno?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetCapabilities
- https://irig-storage.madeira.gov.pt/irig-catalogo/MDT_2m.zip
- https://iris.unive.it/retrieve/5323c7fc-6ba5-4e6c-beac-b08077a52bb6/Abballe%20et%20al.%202025_Novel%20Airborne%20LiDAR-Derived%20Digital%20Terrain%20Models%20in%20the%20Emilia-Romagna%20Region%20%28Italy%29.pdf
- https://isk.geobasis-bb.de/ows/dgm_wcs?SERVICE=WCS&REQUEST=GetCapabilities&VERSION=2.0.1
- https://jerseyeveningpost.com/news/2020/04/22/mystery-of-low-flying-plane-finally-solved/
- https://laiv.geodaten-mv.de/afgvk/Geotopographie/Download?produkt=DGM1
- https://landesentwicklung.steiermark.at/cms/beitrag/12877342/145230171
- https://lgln-geodaten.niedersachsen.de/startseite/luftbilder_und_3d_produkte/3d_produkte/3d_messdaten/3d-messdaten-142870.html
- https://lvermgeo.rlp.de/geodaten-geoshop/open-data
- https://lvermgeo.rlp.de/geodaten-geoshop/open-data/hinweise-zum-download
- https://lvermgeo.rlp.de/produktinformationen/geotopografie/3d-geodaten/digitale-gelaendemodelle-dgm
- https://lvermgeo.rlp.de/produktinformationen/geotopografie/3d-geodaten/laserpunkte-gelaende-lpg-laserpunkte-objekte-lpo
- https://malta.coverage.wetransform.eu/dtm_1m_2012/ows?SERVICE=WCS&VERSION=2.0.1&REQUEST=DescribeCoverage&COVERAGEID=dtm_1m_2012
- https://map.sitr.regione.sicilia.it/gis/rest/services/modelli_digitali?f=json
- https://mappe.regione.vda.it/pub/geonavitg/geodownload.asp?carta=DTM0508
- https://maps.zh.ch/download/hoehen/2022/
- https://mapterhorn.com/
- https://mapterhorn.com/coverage
- https://mapterhorn.com/data-access
- https://marine.gov.scot/data-owners/apgb-aerial-photography-great-britain
- https://metadata.vlaanderen.be/srv/api/records/242ddabc-3cbc-44f0-9623-bee874b29549
- https://metadata.vlaanderen.be/srv/api/records/82271D4B-EBDF-4289-BBB1-69C1C96223DB
- https://metadata.vlaanderen.be/srv/api/records/f52b1a13-86bc-4b64-8256-88cc0d1a8735
- https://metadaten.geoportal-bw.de/geonetwork/srv/api/records/45f2f73c-a258-442d-9df4-ab688c5efe40
- https://metadati.partout.it/metadata_documents/DTM_2005_2008_ORTOMETRICA.pdf
- https://metadati.partout.it/metadata_documents/DTM_2022_DORA_ORTOMETRICA.pdf
- https://metaportal.rlp.de/gui/html/7c359d75-c876-4078-b489-fcfd3332a8bd
- https://metaportal.rlp.de/gui/html/ab69aa3d-e786-41f8-95dc-7b34abb06c41
- https://metawal.wallonie.be/geonetwork/srv/api/records/3ef17388-54cd-4034-baad-d247243123c2
- https://metawal.wallonie.be/geonetwork/srv/api/records/a004e570-99d6-4fe5-b83d-49b774409278
- https://metawal.wallonie.be/geonetwork/srv/api/records/ab14b035-c9b0-4c79-a2b6-36811fca96a1
- https://monentreprise.gouv.mc/en/news/the-principality-in-three-dimensions
- https://ni-lgln-opengeodata.hub.arcgis.com/api/search/v1/collections/all/items?q=Punktwolke&limit=20
- https://noe.gv.at/noe/Open-Government-Data/Geographie_Planung.html
- https://observador.pt/2026/04/07/governo-dos-acores-lanca-concurso-para-cartografia-topografica-vetorial-de-elevado-detalhe/
- https://opendata.gov.je/api/3/action/package_search?q=lidar&rows=10
- https://opendata.schleswig-holstein.de/api/3/action/package_search?q=DGM1&rows=10
- https://opendata.sitcan.es/api/3/action/package_search?q=lidar&rows=20
- https://opendata.sitcan.es/api/3/action/package_search?q=terreno&rows=20
- https://opendata.sitcan.es/notice
- https://opendata.sitcan.es/upload/244/LDR/laz/244_LDR_LP.laz
- https://opendata.swiss/en/terms-of-use
- https://opendem.info/
- https://opendem.info/opendemeu_background.html
- https://opendem.info/opendemeu_download_highres.html
- https://owsproxy.lgl-bw.de/owsproxy/wcs/WCS_INSP_BW_Hoehe_Coverage_DGM1?SERVICE=WCS&REQUEST=GetCapabilities&VERSION=2.0.1
- https://pnoa.ign.es/web/portal/pnoa-lidar/especificaciones-tecnicas
- https://pnoa.ign.es/web/portal/pnoa-lidar/modelo-digital-del-terreno
- https://pnoa.ign.es/web/portal/pnoa-lidar/primera-cobertura
- https://pnoa.ign.es/web/portal/pnoa-lidar/productos-a-descarga
- https://pnoa.ign.es/web/portal/pnoa-lidar/segunda-cobertura
- https://pnoa.ign.es/web/portal/pnoa-lidar/tercera-cobertura
- https://portal.opentopography.org/
- https://portal.opentopography.org/csw/catalog.json
- https://portalrediam.cica.es/geonetwork/srv/api/records/3d2a3ae0-2819-480a-8754-1b3a92805419
- https://portalrediam.cica.es/geonetwork/srv/api/records/7c94b2b2-52e3-4351-b156-44094f7fe953
- https://portalrediam.cica.es/geonetwork/srv/api/records/fb916b4f-76c4-4d8f-a3d2-4fd3641e509d
- https://raster.sitg.ge.ch/arcgis/rest/services/MNA_TERRAIN_2025_03/ImageServer?f=json
- https://raw.githubusercontent.com/wiesehahn/lidar-availability/main/availability_germany.md
- https://registry.opendata.aws/terrain-tiles/
- https://remotesensingdata.gov.scot/about
- https://service.pdok.nl/rws/ahn/atom/dtm_05m.xml
- https://service.salzburg.gv.at/ogd/client/showDetail/aa7b0528-70c1-42c4-a03f-87d2d6a452d7
- https://servicios.idee.es/wcs-inspire/mdt?REQUEST=GetCapabilities&SERVICE=WCS
- https://servizigis.regione.emilia-romagna.it/wcs/dtmrer2022?request=getcapabilities&service=wcs
- https://servizigis.regione.emilia-romagna.it/wcs/dtmrer2023_24?service=wcs&request=getcapabilities&version=1.0.0
- https://servizigis.regione.emilia-romagna.it/wcs/dtmrer2025_26?service=wcs&request=getcapabilities&version=1.0.0
- https://sg.geodatenzentrum.de/web_public/gdz/aktualitaet/dgm1.html
- https://siatservices.provincia.tn.it/stemdata/2014_lidar_dtm_asc/
- https://sim.mase.gov.it/pst/
- https://sit.regione.campania.it/carta-tecnica-regionale
- https://sitex.juntaex.es/SITEX/centrodescargas/view/5
- https://sitex.juntaex.es/SITEX/centrodescargas/view/6
- https://sma.idea.azores.gov.pt/geonetwork/srv/api/records/291673dc-2636-4d9f-9917-4434a5073588/formatters/xml
- https://sma.idea.azores.gov.pt/geonetwork/srv/api/records/94c6d92d-c986-461f-b822-4fdaa4b80ca4/formatters/xml
- https://snig.dgterritorio.gov.pt/rndg/srv/api/records/077a8c94-8b46-4a8a-8796-0d7fc4662f0c/formatters/xml
- https://snig.dgterritorio.gov.pt/rndg/srv/api/records/abba9a83-cb1c-4d2c-ad1d-ccb592247754/formatters/xml
- https://snig.dgterritorio.gov.pt/rndg/srv/api/records/b93228d48b794d4f8c78759409387c8f/formatters/xml
- https://sonny.4lima.de/
- https://srsp-ows.jncc.gov.uk/ows?service=wms&version=1.3.0&request=GetCapabilities
- https://static.salzburg.gv.at/sagisdaten_download/SAGIS_DATEN/ALS/laserscanabdeckung.zip
- https://suche.transparenz.hamburg.de/api/3/action/package_search?q=Punktwolke&rows=40&fl=name,title,metadata_modified
- https://suche.transparenz.hamburg.de/api/3/action/package_show?id=gelaendemodelle-hamburg-dgm2
- https://theintermediary.co.uk/?p=16369
- https://tiles-eu1.arcgis.com/kswen6BYexuc1SUk/arcgis/rest/services/Topographic_LiDAR_DTM_TPK/MapServer?f=json
- https://tinitaly.pi.ingv.it/Download_Area1_1.html
- https://tirol.gv.at/sicherheit/geoinformation/geodaten-tiris/laserscandaten
- https://tirol.gv.at/sicherheit/geoinformationen-karten/freie-laserscandaten-und-orthofotos
- https://tlbg-onlineshop.thueringen.de/onlineshop/uebersichten/digitale-gelaendemodelle-dgm
- https://urbisdownload.datastore.brussels/atomfeed/1d7bd49d-fe83-4388-af85-6f5dc8ec7909-en.xml
- https://vogis.cnv.at/geonetwork/srv/api/records/bd8bc975-a1e9-4374-811f-5773c7a84e33/formatters/xml
- https://web.archive.org/web/2026/https://geodaten.schleswig-holstein.de/gaialight-sh/_apps/dladownload/dl-dgm1.html
- https://web.archive.org/web/2026/https://geoportal.geoportal-th.de/dienste/atom_th_hoehendaten_las
- https://web.archive.org/web/2026/https://geoportal.geoportal-th.de/gaialight-th/_apps/dladownload/dl-dhm.html
- https://web.archive.org/web/2026/https://geoportal.geoportal-th.de/hoehendaten/Uebersichten/README_Hoehendaten_2020-2025.pdf
- https://web.archive.org/web/2026/https://ideas.asturias.es/en/centro-de-descargas
- https://web.archive.org/web/2026/https://igmi.org/en/descrizione-prodotti/cartografia-digitale/digital-terrain-model-dtm
- https://web.archive.org/web/2026/https://irdat.regione.fvg.it/consultatore-dati-ambientali-territoriali/detail/irdat/dataset/11826
- https://web.archive.org/web/2026/https://mapas.xunta.gal/es/aviso-legal
- https://web.archive.org/web/2026/https://mapas.xunta.gal/es/utilidades-y-publicaciones/actualidad/nueva-cobertura-lidar-2015-2016
- https://web.archive.org/web/2026/https://metaver.de/trefferanzeige?docuuid=C448202A-0F15-413A-99E6-F22F10E190C9
- https://web.archive.org/web/2026/https://tailte.ie/en/surveying/products/professional-mapping/height-data
- https://web.archive.org/web/2026/https://umbriageo.regione.umbria.it/articoli/download-dati-cartografici-000
- https://web.archive.org/web/2026/https://umbriageo.regione.umbria.it/catalogostazioni/catalogo.aspx?cd=149
- https://web.archive.org/web/2026/https://www.geoportale.piemonte.it/geonetwork/srv/api/records/r_piemon:224de2ac-023e-441c-9ae0-ea493b217a8e
- https://web.archive.org/web/2026/https://www.regione.marche.it/Regione-Utile/Paesaggio-Territorio-Urbanistica-Genio-Civile/Cartografia-e-informazioni-territoriali/Repertorio
- https://webgis.regione.sardegna.it/geoserverraster/ows?service=wms&version=1.3.0&request=GetCapabilities
- https://wp.unil.ch/risk/andorra-la-vella-lidar-monitoring/
- https://www.ag.ch/de/verwaltung/dfr/geoportal/geodaten/geodatenliste?rewriteRemoteUrl=/details/AGIS.KAI_LiDARDTM24
- https://www.ahn.nl/
- https://www.ahn.nl/dataroom
- https://www.ahn.nl/eerste-deel-ahn-6-beschikbaar
- https://www.ahn.nl/kwaliteitsbeschrijving
- https://www.ahn.nl/uitgelicht
- https://www.ahn.nl/waar-vliegen-we-nu
- https://www.asita.it/wp-content/uploads/2024/12/poster_vincenzi_asita.pdf
- https://www.bafg.de/DE/5_Informiert/4_Infothek/Aktuelles/_doc/2025/251121_DGM-W-Elbe.html
- https://www.berlin.de/sen/stadt/stadtdaten/geoinformation/landesvermessung/geotopographie-atkis/dom-digitales-oberflaechenmodell/
- https://www.bev.gv.at/Services/Produkte/Digitales-Gelaendehoehenmodell/ALS-Hoehenraster.html
- https://www.bezreg-koeln.nrw.de/geobasis-nrw/produkte-und-dienste/hoehenmodelle/3d-messdaten
- https://www.bezreg-koeln.nrw.de/geobasis-nrw/produkte-und-dienste/hoehenmodelle/digitale-gelaendemodelle/digitales-gelaendemodell
- https://www.bizkaia.eus/es/web/geobizkaia/recursos
- https://www.bluesky-world.com/
- https://www.cartografia.ad/serveis-ogc
- https://www.cartografia.ad/web-de-descarregues
- https://www.cartografia.servizirl.it/download/DTM5_RL.zip
- https://www.coigt.com/noticia/3435
- https://www.data.gouv.fr/api/1/datasets/61489f5ae64c7f138f1b2e15/
- https://www.data.gouv.fr/api/1/datasets/675871bd6e60265b46ab356e/
- https://www.data.gouv.fr/api/1/datasets/67f59d2966c82acad240a1cb/
- https://www.data.gouv.fr/api/1/datasets/6836523318d64a73d170837a/
- https://www.data.gouv.fr/api/1/datasets/?q=BD+ALTI&page_size=8
- https://www.data.gouv.fr/api/1/datasets/?q=lidar&page_size=50
- https://www.data.gouv.fr/api/1/datasets/?q=lidar&page_size=50&page=2
- https://www.data.gov.uk/api/action/package_search?q=lidar+composite+dtm&rows=15
- https://www.data.gov.uk/api/action/package_show?id=lidar-composite-digital-terrain-model-dtm-1m
- https://www.data.gov.uk/api/action/package_show?id=national-lidar-programme
- https://www.data.gov.uk/api/action/package_show?id=scottish-coastal-lidar-survey-2026-dtm
- https://www.data.gov.uk/api/action/package_show?id=scottish-land-lidar-programme-dtm
- https://www.data.gv.at/api/hub/search/search?q=ALS%20DTM%20H%C3%B6henraster%201m%20Stichtag&limit=1000
- https://www.data.gv.at/api/hub/search/search?q=K%C3%A4rnten%20Gel%C3%A4ndemodell&limit=300
- https://www.dati.gov.it/iodl/2.0/
- https://www.digimap.gg/
- https://www.earthdata.nasa.gov/data/alerts-outages/digital-surface-terrain-models-from-vantors-precision3d-product-line-added
- https://www.eorc.jaxa.jp/ALOS/en/dataset/aw3d30/aw3d30_e.htm
- https://www.generic-mapping-tools.org/GMTjl_doc/tutorials/dgt_lidar/tut_dgt_lidar.html
- https://www.geo.bremen.de/produkte/3d-produkte/hoehenmodelle-12482
- https://www.geo.bremen.de/produkte/open-data-produktuebersicht-15654
- https://www.geo.euskadi.eus/WCS_KARTOGRAFIA?request=getcapabilities&service=wcs
- https://www.geo.euskadi.eus/lidar/DatosDescarga/LIDAR/
- https://www.geoconnexion.com/news/bluesky-publishes-first-online-3d-maps-of-isle-of-man
- https://www.geodaten-mv.de/dienste/dgm_atom
- https://www.geodaten.bayern.de/download/uebersicht_DGM/Laserscanningbefliegungen.kml
- https://www.geodaten.bayern.de/odd/m/3/html/datenhinweise/datenhinweise_dgm.html
- https://www.geodaten.bayern.de/odd/m/3/html/nutzungsbedingungen.html
- https://www.geodaten.sachsen.de/batch-download-4719.html
- https://www.geodaten.sachsen.de/downloadbereich-digitale-hoehenmodelle-4851.html
- https://www.geoportale.regione.lombardia.it/en/note-legali
- https://www.getmapping.com/
- https://www.gibraltar.gov.gi/press-releases/department-of-the-environment-heritage-climate-change-invests-in-unmanned-aerial-vehicle-2422019-4808
- https://www.gibraltar.gov.gi/press-releases/northern-defences-lidar-survey-and-current-works-8552023-9460
- https://www.gov.je/PlanningBuilding/JerseyMappingService/Pages/Digital.aspx
- https://www.gov.sm/pub1/GovSM/Dipartimenti/Dipartimento-Territorio-e-Ambiente/Ufficio-Tecnico-del-Catasto-e-Cartografia.html
- https://www.govdata.de/dl-de/by-2-0
- https://www.govdata.de/dl-de/zero-2-0
- https://www.grafcan.es/servicios/lidar-2/
- https://www.grafcan.es/servicios/ventas-y-descargas/
- https://www.hamburg.de/politik-und-verwaltung/behoerden/behoerde-fuer-stadtentwicklung-und-wohnen/aemter-und-landesbetrieb/landesbetrieb-geoinformation-und-vermessung/wir-ueber-uns/laserscanning-und-vermessungsbefliegungen-1136924
- https://www.icgc.cat/en/Geoinformation-and-Maps/Data-and-products/Elevations/Territorial-elevations/Elevation-models
- https://www.icgc.cat/en/Geoinformation-and-Maps/Data-and-products/Elevations/Territorial-elevations/Territorial-LiDAR
- https://www.icgc.cat/en/ICGC/News/We-have-completed-third-LiDAR-coverage-Catalonia
- https://www.ideandorra.ad/Cataleg/srv/cat/q?any=MDT&fast=index&to=50
- https://www.ideandorra.ad/Cataleg/srv/cat/q?any=lidar&fast=index&to=50
- https://www.idee.es/csw-codsi-idee/srv/api/records/md_ideeu_mdt_lidar_2017_es/formatters/xml
- https://www.igeoe.pt/
- https://www.intermap.com/nextmap
- https://www.laiv-mv.de/Geoinformation/Geobasisdaten/Oberflaechenmodelle/
- https://www.laiv-mv.de/static/LAIV/Geoinformation/Dateien/Geobasisdaten/Kurzbeschreibung_ATOM_DGM.pdf
- https://www.landesentwicklung.steiermark.at/cms/beitrag/12803182/142970647/
- https://www.landesvermessung.sachsen.de/allgemeine-nutzungsbedingungen-8954.html
- https://www.landesvermessung.sachsen.de/download-dienste-und-testdaten-8647.html
- https://www.landesvermessung.sachsen.de/fachliche-details-8645.html
- https://www.landesvermessung.sachsen.de/planung-aktualitaet-8643.html
- https://www.landesvermessung.sachsen.de/prod_dhm_stadt/dhm_stadt.pdf
- https://www.ldbv.bayern.de/produkte/landschaftsinformationen/gelaende.html
- https://www.ldbv.bayern.de/produkte/landschaftsinformationen/laser.html
- https://www.lgl-bw.de/Open-GeoData
- https://www.lgl-bw.de/Produkte/3D-Produkte/Digitale-Gelaendemodelle/index.html
- https://www.lgl-bw.de/Produkte/3D-Produkte/Laserscandaten/ALS_2/
- https://www.lgl-bw.de/Produkte/3D-Produkte/Laserscandaten/ALS_3/
- https://www.lgln.niedersachsen.de/startseite/wir_uber_uns_amp_organisation/allgemeine_geschafts_und_nutzungsbedingungen_agnb/allgemeine-geschafts-und-nutzungsbedingungen-agnb-97401.html
- https://www.lvermgeo.sachsen-anhalt.de/de/datei/download/id/17600,501/dgm1_st_aktualitaetsuebersicht_2025_12_05.pdf
- https://www.lvermgeo.sachsen-anhalt.de/de/datei/download/id/416049,501/laserscan_befliegungen_st.pdf
- https://www.lvermgeo.sachsen-anhalt.de/de/gdp-3d-messdaten.html
- https://www.lvermgeo.sachsen-anhalt.de/de/gdp-dgm-dom-lsa.html
- https://www.lvermgeo.sachsen-anhalt.de/de/gdp-open-data.html
- https://www.maptiler.com/cloud/terms/
- https://www.mase.gov.it/pagina/cig-b2bc00915e-affidamento-del-servizio-di-realizzazione-del-modello-digitale-del-terreno
- https://www.nationalarchives.gov.uk/doc/open-government-licence/version/3/
- https://www.opengeodata.nrw.de/produkte/geobasis/hm/
- https://www.opengeodata.nrw.de/produkte/geobasis/hm/3dm_l_las/3dm_l_las/
- https://www.opengeodata.nrw.de/produkte/geobasis/hm/3dm_l_las/3dm_l_las/3dm_meta.zip
- https://www.opengeodata.nrw.de/produkte/geobasis/hm/dgm1_tiff/dgm1_tiff/
- https://www.provincia.tn.it/News/Approfondimenti/Rilievo-Lidar-del-territorio-della-Provincia-di-Trento
- https://www.regione.lombardia.it/ambiente-e-territorio/infrastruttura-dati-territoriali-idt/consultazione-e-distribuzione-delle-foto-aeree-e-dei-rilievi-lidar
- https://www.regione.marche.it/Portals/0/Paesaggio_Territorio_Urbanistica/Cartografia/Metadati/Relazione_DTM.pdf
- https://www.rivistageomedia.it/geodata/portale-pst-mase-aggiornamento-lidar-interferometria-2026
- https://www.rivistageomedia.it/survey-and-positioning/al-via-la-realizzazione-del-nuovo-modello-altimetrico-del-territorio-italiano
- https://www.schleswig-holstein.de/DE/Landesregierung/LVERMGEOSH/Service/serviceGeobasisdaten/geodatenService_Geobasisdaten_DGM.html
- https://www.shop.lvgl.saarland.de/index.php?option=com_content&view=article&id=18&metaauthor=odata
- https://www.sig.strasbourg.eu/datastrasbourg/3d_fiches_metadonnees_pdf/2025/metadata_nuage_de_points.pdf
- https://www.sitr.regione.sicilia.it/licenza-duso/
- https://www.sitr.regione.sicilia.it/riprese-aereee/volo-anni-2012-2013/
- https://www.swisstopo.admin.ch/en/height-model-swissalti3d
- https://www.swisstopo.admin.ch/en/height-model-swisssurface3d
- https://www.swisstopo.admin.ch/en/height-model-swisssurface3d-raster
- https://www.swisstopo.admin.ch/en/height-model-swisssurface3d-update-20260217
- https://www.swisstopo.admin.ch/en/lidar-data-swisstopo
- https://www.swisstopo.admin.ch/en/terms-of-use-free-geodata-and-geoservices
- https://www.vd.ch/territoire-et-construction/cadastre-et-geoinformation/geodonnees/altimetrie-lidar
- https://www.viewfinderpanoramas.org/dem3.html
- https://www.vlaanderen.be/digitaal-vlaanderen/nieuws/dit-jaar-start-de-vernieuwing-van-het-digitaal-hoogtemodel
- https://www.vlaanderen.be/digitaal-vlaanderen/onze-diensten-en-platformen/earth-observation-data-science-eodas/het-digitaal-hoogtemodel/digitaal-hoogtemodel-vlaanderen-ii
- https://www.vlaanderen.be/digitaal-vlaanderen/onze-oplossingen/open-data/voorwaarden-voor-het-hergebruik-van-overheidsinformatie
- https://www.vlaanderen.be/geopunt/kaarttoepassingen/eodas-open-lidar-portaal
- https://www.wien.gv.at/ma41datenviewer/downloads/geodaten/dgm_tif/35_4_dgm_tif.zip
- https://www.wien.gv.at/stadtplanung/geodatenviewer-ogd-daten-herunterladen
- https://www.zh.ch/de/news-uebersicht/mitteilungen/2023/planen-bauen/geoinformation/aktuelle-hoehendaten-fuer-den-kanton-zuerich.html
- https://www502.regione.toscana.it/geonetwork/srv/api/records/r_toscan:c5c907bf-50cc-4a76-8787-abc547509c8d/formatters/xml
- https://www502.regione.toscana.it/geonetwork/srv/api/records/r_toscan:f3ff89ee-49fa-4487-89fe-c69ae58ecd0c/formatters/xml
- https://www502.regione.toscana.it/geoscopio/geometa/html/r_toscan_261ec1b2-bf25-4973-b853-ac08e060ec62.htm
- https://zenodo.org/api/records/17378779
- https://zenodo.org/api/records/18887460
- https://zenodo.org/api/records?q=GEDTM30&size=3
