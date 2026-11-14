# Highest-resolution terrain data for Northern and Eastern Europe: Nordics, Baltics, Central Europe, Balkans, SE Europe, Turkey, Caucasus, Ukraine, Belarus, Moldova, Russia, non-governmental

Run date 2026-11-12/13 (board item i1312). Read-only research for Scenic Roads (personal offline map). Method: every claim carries the URL of a page or API answer fetched with a 2xx answer in this run (by me or by four sub-agents I briefed, who reported their cited URLs; each agent's report states that its URLs answered 2xx). Search results were used only to discover URLs. Requests used the User-Agent `scenic-roads/0.1 (personal offline map)`. No data was downloaded (HEAD, capabilities, metadata and small page reads only). Wayback copies are marked "(Wayback)". Where a live host timed out and only a Wayback copy was read, the claim says so.

Verdict codes (as in terrain-europe-w.md): NATIONAL-OPEN = the national source is the best open one; SUBNATIONAL-BETTER = a regional/city source that is open beats the national one; RESTRICTED = finer data exists but is paid, request-only, login-gated, public-bodies-only or view-only; GLOBAL-ONLY = nothing found that beats the global products. "pts/m2" = lidar points per square metre. The pan-European and global products (Copernicus GLO-30/GLO-90, EEA-10, EU-DEM, FABDEM, GEDTM30, AW3D30, MERIT, Mapzen/Tilezen) are in section 4 of terrain-europe-w.md and are not repeated; only what differs for this region is below.

## 1. Headline

- Open 1 m or finer national terrain exists for: Norway (1 m DTM and DSM, lidar >= 2 pts/m2, CC BY 4.0), Sweden (1 m DTM COGs on an open STAC), Denmark (0.4 m, CC BY 4.0, but the official route needs a free account), Estonia (1 m DTM/DSM and LAZ, 2021-2024 flights), Poland (1 m NMT and LAZ, free), Slovakia (1 m, with a 0.5 m second cycle under way, CC BY 4.0), Slovenia (ARSO 1 m DMR; 2023-25 CLSS flight at 10 pts/m2) and Cyprus (a 2019 DTM that Mapterhorn republishes at 1 m, CC BY 4.0).
- Open but coarser than 1 m: Finland (2 m DEM; its 5 pts/m2 lidar is paid, the open 0.5 pts/m2 lidar is partial), Iceland (2 m, but a satellite DSM from ArcticDEM), Czechia (DMR 5G TIN from lidar plus a 5 m grid, error 0.18 m open terrain), Faroe Islands (10 m DTM, 2 m DSM from ArcticDEM), Svalbard (5 m partial, 20 m everywhere) and Jan Mayen (5 m).
- Restricted: Hungary (5 m DDM5 is sold; 25 m free), Croatia (nationwide 2022-23 lidar at >= 8 pts/m2, but sold on request; a registry entry says "no conditions", conflict unresolved), Serbia (paid), Federation of BiH (lidar 1 m 2018 river corridors, tariff/limited use), Republika Srpska (2026 DTM layers at 10 m to 0.2 m, token-flagged WMS), North Macedonia (national lidar delivered, "will be public", not yet), Albania (2015-17 lidar DTM/DSM, only a DSM layer in the geoportal), Greece (DTMs on application; 1 m lidar programme in progress), Turkey (HGM sells 5 m DSM for 144 TL; best products public-bodies only), Latvia (open 4 pts/m2 lidar points of 2013-2019, but 1 m derived grids and the newer lidar only for public bodies), Lithuania (a national lidar of 6.5-15 pts/m2, 2019-2026, is catalogued with LAZ sheets, but its licence and open access are not stated and the geoportal ordering needs a login), Romania's lidar (state-secret rules above 9 pts/m2), Russia (federal fund: request, contract, fee).
- Nothing finer than GLO-30 found open: Ukraine, Belarus, Montenegro, Kosovo, Georgia, Armenia, Azerbaijan, Northern Cyprus, the British Sovereign Base Areas, Bulgaria (a view-only WMS), Brčko District; Moldova has a 2007 5 m DTM but only as WMS images. For Ukraine, Belarus and Russia the "nothing found" is an absence of evidence (many hosts timed out or were blocked; see section 5).
- Subnational beats national in these places found: Prague (IPR Praha lidar DTM, about 40 pts/m2, CC BY) and Brno (0.25 m DTM 2019, CC BY) over ČÚZK; Gothenburg (0.5 m, 10 pts/m2, CC0, 2022) over Lantmäteriet's 1 m; Helsinki (1 m WCS, free) over NLS's 2 m; Stockholm (16 pts/m2 cloud 2025 and 1 m grid, but paid); Sofia (a downloadable 5 m DTM against Bulgaria's view-only WMS, no licence stated); Republika Srpska's 2026 layers (not open); Moldova's regional 2020/2021 DTM layers (view-only). Warsaw and Kraków lidar is published nationally by GUGiK (free). Oslo, Copenhagen, Reykjavik, Tallinn, Ljubljana, Zagreb, Athens, Thessaloniki, Kyiv, Yerevan and Nicosia showed nothing better than the national source; Vilnius, Budapest, Bratislava, Belgrade, Bucharest, Chişinău, Minsk, Ankara, Istanbul, Tbilisi, Baku, Moscow, St Petersburg, Brčko and the Russian subjects (Tatarstan, Sakhalin, Kamchatka, Yakutia) could not be reached or showed no product (section 2.5 and 5). Poland's voivodeships and Hungary's counties were not checked.
- Non-governmental: Mapterhorn already carries open national DTMs from this region as PMTiles (section 4), the most useful offline-friendly channel. ArcticDEM (2 m DSM, CC-BY-4.0 in its STAC collection) is the open fallback for the high north (Svalbard, Iceland, Faroes, Arctic Russia).
- Copernicus GLO-30 was opened to Armenia, Azerbaijan and Moldova in a Dec 2023 release per the CDSE page, so the "restricted countries" gap there is closed.

## 2. Summary table (one row per jurisdiction)

Columns: jurisdiction | verdict | best source | grid / density | DTM or DSM | year | licence | access | source URL.

### 2.1 Nordics and Baltics

| Jurisdiction | Verdict | Best source | Grid / density | DTM/DSM | Year | Licence | Access | URL |
|---|---|---|---|---|---|---|---|---|
| Norway (mainland) | NATIONAL-OPEN | Kartverket NDH (hoydedata.no) | 1 m; lidar >= 2 pts/m2 (5 in selected areas) | DTM and DSM | project 2016-2022, rolling | CC BY 4.0 | bulk, WCS/WMS/WFS, REST download | https://www.kartverket.no/en/api-and-data/terrengdata |
| Svalbard | NATIONAL-OPEN (partial 5 m) | Norsk Polarinstitutt Svalbard DTM 5; DTM 20 whole land | 5 m partial (2 m Bjørnøya); 20 m all land | DTM | DTM5 updated 2023-11-08 | CC BY 4.0 | Geonorge download API | https://kartkatalog.geonorge.no/api/getdata/010bfb4a-65aa-4d30-aba9-7d090a432df7 |
| Jan Mayen | NATIONAL-OPEN | NPI Jan Mayen DTM 5 | 5 m | DTM | published 2024-10-28 | CC BY 4.0 | Geonorge download API | https://kartkatalog.geonorge.no/api/getdata/731a0602-d262-483d-aaa6-474cef68e82a |
| Sweden | NATIONAL-OPEN | Lantmäteriet Markhöjdmodell (STAC dtm-cog); Laserdata Skog (COPC) | 1 m COG, 10 km tiles | DTM (TIN of ground points) | 2009-08 to 2026-07 | STAC: CC-BY-4.0 (open-data page says CC0) | STAC API, no login | https://api.lantmateriet.se/stac-hojd/v1/collections/dtm-cog |
| Finland (mainland) | NATIONAL-OPEN (2 m); finer lidar paid | NLS Elevation model 2 m; lidar 0.5 p (open, partial) and 5 p (paid) | 2 m; 0.5 pts/m2 open; 5 pts/m2 paid | DTM | yearly additions (~40 000 km2/yr) | CC BY 4.0 (2 m, 0.5 p) | MapSite file download; WCS and file service (OGC API Processes) need a free API key from NLS "My Account" (NLS key page) | https://www.maanmittauslaitos.fi/en/maps-and-spatial-data/datasets-and-interfaces/product-descriptions/elevation-model-2-m |
| Åland | NATIONAL-OPEN (NLS data) | NLS 2 m DEM; the Åland government has only layers derived from it | 2 m | DTM | 2013 and 2015 | CC BY 4.0 (NLS) | as Finland | https://kartor.regeringen.ax/arcgis/rest/services/Topografi/Hojd/MapServer?f=json |
| Denmark | NATIONAL-OPEN (free account) | Klimadatastyrelsen DK-DEM / DHM | 0.4 m (also 1, 5, 20 m); 4-5 pts/m2 | DTM and DSM | scanned 2014-15 (newer version reported 2025 by a third party) | CC BY 4.0 | Dataforsyningen token (INSPIRE OGC services need none) | https://eng.kds.dk/green-transition-and-climate-adaptation/the-danish-elevation-model-dk-dem |
| Faroe Islands | NATIONAL-OPEN (low-res) | Umhvørvisstovan via føroyakort.fo | DSM 2/5/10 m; DTM 10 m | DSM from ArcticDEM; DTM from 2009 photos | 2015 / 2009; files 2024 | Faroese open data (attribution) | direct download, ArcGIS REST | https://www.foroyakort.fo/tak-datur-nidur/dtm-og-dsm-haeddarmodell/ |
| Iceland | NATIONAL-OPEN (satellite) | Náttúrufræðistofnun ÍslandsDEM v1.0 | 2 m (10/20/50 m on FTP) | DSM (ArcticDEM median mosaic) | dated 2020-02-23 | CC BY 4.0 | viewer (2 m), WMS, FTP | https://gatt.natt.is/geonetwork/srv/api/records/e6712430-a63c-4ae5-9158-c89d16da6361/formatters/xml |
| Estonia | NATIONAL-OPEN | Maa- ja Ruumiamet ALS | DTM/DSM 1 m (5, 10, 25 m too); LAZ 1 km2 | DTM and DSM | newest DEM from 2021-2024 flights | Estonian Open Data Licence | per-sheet and whole-country download | https://geoportaal.maaamet.ee/eng/spatial-data/elevation-data-p308.html |
| Latvia | RESTRICTED (points open, 1 m grids public-bodies only) | LĢIA lidar cycle 1 (LAS) | >= 4 pts/m2 (ground >= 1.5); open DTM 20 m | classified points | 2013-2019 (cycle 2 2022-24 not open) | open-data licence | LAS tile download | https://www.lgia.gov.lv/en/digital-height-models-0 |
| Lithuania | RESTRICTED (national lidar exists; terms and open access not confirmed) | NŽT "Lidar_DR_LT" national lidar point cloud (geoportal.lt metadata catalogue); older county SEŽP points | >= 6.5 pts/m2 (2019-2025 flights); >= 15 pts/m2 (2026 flight); RMSE vertical <= 10 cm | classified points (class 2 ground), LAZ in LKS-94 sheets | 2019-2026 (revised 2026-08) | record says only "copyright" / "license", no text; terms unread | LAZ sheets, one download URL per sheet; ordering on geoportal.lt needs a login | https://www.geoportal.lt/metadata-catalog/csw?service=CSW&version=2.0.2&request=GetRecordById&elementSetName=full&outputSchema=http://www.isotc211.org/2005/gmd&id=%7B3C0D5E47-49CA-4132-88C1-EC3D1E4610B4%7D |

### 2.2 Central Europe and the Balkans

| Jurisdiction | Verdict | Best source | Grid / density | DTM/DSM | Year | Licence | Access | URL |
|---|---|---|---|---|---|---|---|---|
| Poland | NATIONAL-OPEN | GUGiK NMT / NMPT / LAZ | 1 m NMT (5 m too); ALS 4-20 pts/m2 | NMT DTM, NMPT DSM | cycle ongoing | free, "dowolne wykorzystanie" | per-sheet download, WMS/WMTS, WFS indexes (hosts timed out) | https://www.geoportal.gov.pl/pl/dane/numeryczny-model-terenu-nmt/ |
| Czechia | NATIONAL-OPEN | ČÚZK DMR 5G (TIN) + DMR 4G (5 m grid); DMP 1G surface | TIN; 5 m grid | DTM (DMP 1G DSM) | ALS 2009-2013, whole country by 2016-06-30 | CC BY 4.0 | ATOM, WMS, sheet files | https://geoportal.cuzk.gov.cz/(S(1))/Default.aspx?mode=TextMeta&side=dmr5g&metadataID=CZ-CUZK-DMR5G-V&menu=302 |
| Prague | SUBNATIONAL-BETTER (city) | IPR Praha DTM raster | lidar ~40 pts/m2 (cell size not read) | DTM | not stated | CC BY | prepared packages | https://www.arcgis.com/sharing/rest/content/items/97d2c9c11aa9478cb21b469b8a4f820e?f=json |
| Slovakia | NATIONAL-OPEN | ÚGKK/GKÚ LLS DMR 5.0 / DMP 1.0 (cycle 1); DMR 6.0 / DMP 2.0 (cycle 2) | 1 m (>= 5 pts/m2); 0.5 m (>= 15 pts/m2) | DTM and DSM | 2017-2023; 2022-2026 in progress | CC BY 4.0 | whole-country ZIP, per-lot ZIPs, MAPKA clips | https://www.skgeodesy.sk/geoportal/zbgis/lls/poskytovanie/ |
| Hungary | RESTRICTED | Lechner Tudásközpont DDM5 | 5 m (10, 25 m offered) | DTM from contours + photogrammetry | refreshed 2000/2005/2015/2022 | priced (25 m free) | paid; GeoTIFF/XYZ/WMS | https://lechnerkozpont.hu/node/1963 |
| Slovenia | NATIONAL-OPEN | ARSO LSS DMR (1 m) and GURS CLSS 2023-25 (10 pts/m2) | 1 m DMR; Mapterhorn lists country-wide 0.5 m | DTM and DSM | LSS 2011-2015; CLSS 2023-2025 | CC BY 4.0 (ARSO per Mapterhorn; GURS DMV 5 m CC BY 4.0) | clss.si viewer; sheet download (terms unread) | https://web.archive.org/web/2026/https://eprostor.gov.si/imps/srv/api/records/951eaa88-0f6b-4d29-8d02-eaf56534453e/formatters/xml |
| Croatia | RESTRICTED | DGU nationwide lidar DMR/DMV | lidar >= 8 pts/m2; DMV ~1 m | DTM (DSM also) | lidar 2022-23 | DGU DMR page: fee per cost regulation; NIPP registry entry for the lidar cloud: "Nema uvjeta za pristup i korištenje" (see note) | written request | https://dgu.gov.hr/proizvodi-i-usluge/podaci-topografske-izmjere/digitalni-model-reljefa/180 |
| Bosnia and Herzegovina: Federation of BiH | RESTRICTED | FGU/IPP FBiH lidar DTM and DMR 5 m | 1 m, 4-20 pts/m2, 10 cm abs. height accuracy (river corridors); 5 m national | DTM | 2018; 2012 | limited use per regulations and tariffs | request / tariff | https://metapodaci.ippfbih.gov.ba/srv/api/records/2cfd6c83-748e-45ec-acc8-a9729e3173dc |
| Bosnia and Herzegovina: Republika Srpska | RESTRICTED | RGURS geoportal "Digitalni model terena (2026)" | 10 m, 5 m, 1 m, 50 cm, 20 cm for 20 local units | DTM | 2026; 2012 | not stated | WMS, group flagged keycloakToken | https://geoportal.rgurs.org/init/geoportal.json |
| Bosnia and Herzegovina: Brčko District | GLOBAL-ONLY | none public found | n/a | n/a | n/a | n/a | n/a | https://www.klix.ba/scitech/nauka/bosna-i-hercegovina-uskoro-dobija-prvu-jedinstvenu-3d-kartu-zemlje-zavrseno-lidar-snimanje/260825121 |
| Serbia | RESTRICTED | RGZ DMT; Tisa lidar DTM | 5, 10, 25 m; 1 m (Tisa floodplain) | DTM | 2007-2013; lidar 2012 | state fees law | paid / request | https://www.rgz.gov.rs/digitalni-model-terena |
| Montenegro | GLOBAL-ONLY | none found (2010 trade news of a lidar DTM contract) | n/a | n/a | n/a | n/a | n/a | https://www.gim-international.com/content/news/dtp-and-dop-for-entire-montenegro |
| Kosovo | GLOBAL-ONLY | none found | n/a | n/a | n/a | n/a | n/a | https://www.kca-ks.org/ |
| North Macedonia | RESTRICTED (planned public) | AKN lidar project (with Norwegian Kartverket) | point cloud + DTM + DSM over 11 072 km2 | DTM and DSM | project from 2018 | not stated | lidar portal (Wayback) | https://web.archive.org/web/2026/https://aknlidarappprod.katastar.gov.mk/LiDARInfoStrana/index.html |
| Albania | RESTRICTED | ASIG 2015-17 lidar DTM/DSM | 5 / 2 / <=1 pts/m2 (urban / rural / mountain) | DTM and DSM | 2015-2017 | not found | geoportal (DSM layer) | https://asig.gov.al/en/aerial-photography-in-the-republic-of-albania/ |

### 2.3 SE Europe, Turkey, Caucasus

| Jurisdiction | Verdict | Best source | Grid / density | DTM/DSM | Year | Licence | Access | URL |
|---|---|---|---|---|---|---|---|---|
| Romania | NATIONAL-OPEN (coarse) + partial 0.5 m | ANCPI MNT; LAKI II MNT 0.5 m (Mapterhorn) | national grid size not stated (component DTMs 1, 2, 5 m); LAKI II 0.5 m partial | DTM | contours 2014-15, update 2019-20; LAKI II 2019-22 | OGL-ROU-1.0 | geoportal (unreachable here) | https://help.emd.dk/mediawiki/images/c/ca/Licen%C8%9Ba_deschisa_Modelul_numeric_al_terenului_v2_MNT.pdf |
| Bulgaria | GLOBAL-ONLY (view-only WMS) | Ministry of Agriculture INSPIRE "Orthophoto and DEM View Service" | not stated | DTM 2023-25 layers; DSM 2020-22 | 2020-2025 | "no conditions" but view service | WMS only; Sofia city (see 2.5) has a downloadable 5 m DTM | http://inspire.mzh.government.bg:8080/geoserver/ows?service=wms&version=1.3.0&request=GetCapabilities |
| Greece | RESTRICTED | Hellenic Cadastre DTMs on application; 1 m lidar programme planned | 1 m planned | DTM and DSM | 2024-26 | unknown | application | https://web.archive.org/web/2025/https://www.gov.gr/ipiresies/periousia-kai-phorologia/ktematographese/geopule-geokhorikon-dedomenon-inspire-ellenikou-ktematologiou |
| Cyprus (Republic) | NATIONAL-OPEN (registration, 1 GB/day free) | DLS photogrammetric e-service; Mapterhorn "Cyprus DTM 2019" | 1 m per Mapterhorn; page states none | DTM and DSM | 2019 | CC BY 4.0 (Mapterhorn attribution) | e-service login; Mapterhorn PMTiles | https://portal.dls.moi.gov.cy/en/?p=18685 |
| Northern Cyprus | GLOBAL-ONLY | nothing published found (2025 Turkish tender for 3D work) | n/a | n/a | n/a | n/a | n/a | https://sabah.com.tr/ekonomi/kktcnin-dijital-ikizi-cikarilacak-7367458 |
| Sovereign Base Areas (UK) | GLOBAL-ONLY | nothing found | n/a | n/a | n/a | n/a | n/a | https://portal.dls.moi.gov.cy/en/?p=18685 (does not say if SBAs are included) |
| Turkey | RESTRICTED | HGM SYM5-L0 (5 m, 144 TL), SAM 30 m, DTED-2; SYM12/SAM12 free for public bodies only | 5 m | SYM5 DSM; SAM DTM | SYM12 2011-14 | sale terms | emailed order form (Wayback) | https://web.archive.org/web/2025/https://www.harita.gov.tr/urunler/yukseklik-verileri-grid/1 |
| Georgia | GLOBAL-ONLY | none found | n/a | n/a | n/a | n/a | n/a | https://napr.gov.ge/ |
| Armenia | GLOBAL-ONLY (national set unpublished) | Cadastre Committee 2014 digital relief model | not stated | not stated | 2014 | not stated | not stated | https://cadastre.am/en/cadastre_mapping |
| Azerbaijan | GLOBAL-ONLY | none found | n/a | n/a | n/a | n/a | n/a | https://dataspace.copernicus.eu/explore-data/data-collections/copernicus-contributing-missions/collections-description/COP-DEM (fallback only) |

### 2.4 Ukraine, Belarus, Moldova, Russia

| Jurisdiction | Verdict | Best source | Grid / density | DTM/DSM | Year | Licence | Access | URL |
|---|---|---|---|---|---|---|---|---|
| Ukraine (and Crimea) | GLOBAL-ONLY | GLO-30 (tiles over Kyiv and Crimea exist) | 30 m | DSM | 2011-15 base | Copernicus | S3 COG | https://registry.opendata.aws/copernicus-dem/ |
| Belarus | GLOBAL-ONLY (unconfirmed) | GLO-30; the cadastre agency's orthophoto service names no DEM | 30 m | DSM | as above | Copernicus | S3 COG | https://ggim.un.org/meetings/2019/IWS/Minsk/documents/S2-7%20Belarus-Marina%20Litreeva.pdf |
| Moldova | RESTRICTED (WMS images only) | AGCC/INGEOCAD "Modelul Digital al Terenului 2007" (+ regional 2020, 2021 DTMs) | 5 m layer | DTM | 2007; 2020 (centre); 2021 (south) | INSPIRE: no limitations / no conditions | WMS render; WCS and WFS "disabled" | https://geodata.gov.md/geoserver/ows?service=WMS&request=GetCapabilities |
| Transnistria | GLOBAL-ONLY | 2007 DTM excludes the left bank; GLO-30 | 30 m | DSM | | Copernicus | | https://web.archive.org/web/20260510121956/https://inspire-geoportal.ec.europa.eu/srv/api/records/38dac355-7cc1-4110-ab04-7406e3e5fbb5 |
| Russia (federal) | RESTRICTED (request, contract, fee) | FGBU "Centr geodezii, kartografii i IPD" (FFPD) | not stated | not stated | n/a | n/a | application + contract + fee (Wayback) | https://web.archive.org/web/20241206110844/https://cgkipd.ru/fsdf/service/provide/ |
| Russia north (Arctic) | NATIONAL: n/a; ArcticDEM | ArcticDEM mosaics v4.1 | 2 m (10, 32 m) | DSM | imagery 2007-2024 | CC-BY-4.0 (STAC collection) | bulk, STAC, ImageServer | https://pgc-opendata-dems.s3.us-west-2.amazonaws.com/arcticdem/mosaics.json |
| Russia (rest) and federal subjects (Moscow, St Petersburg, Moscow oblast, Tatarstan, Krasnodar) | GLOBAL-ONLY (home pages only; unverified) | GLO-30 (Moscow and Murmansk tiles exist) | 30 m | DSM | | Copernicus | S3 COG | https://web.archive.org/web/20260106145652/https://fpd.tatarstan.ru/ |

### 2.5 Cities and regional bodies checked (one geoportal / open-data visit plus one search each)

| Unit | Verdict | Product | Grid / density | DTM/DSM | Year | Licence | Access | URL |
|---|---|---|---|---|---|---|---|---|
| Brno | CITY-BETTER | City "Digitální model terénu 2019" (ImageServer) | 0.25 m pixel; lidar density not stated | DTM (elevation ImageServer; contours derived) | 2019 | CC BY (record: licence_name "CC BY") | ArcGIS ImageServer; TIFF/XYZ/SHP/DWG downloads | https://gis.brno.cz/ags1/rest/services/OMI/OMI_DMT_2019_elevation/ImageServer?f=json |
| Gothenburg | CITY-BETTER | Göteborgs Stad "Höjdmodell 2022" | 0.5 m GeoTIFF; 10 pts/m2 | not stated on the sheet (height under buildings interpolated) | flown May 2022 | CC0 | zip downloads via goteborg.se open-data page | https://opengeodata.goteborg.se/files/hojdmodell/2022/hojdmodell_2022_sv.pdf |
| Stockholm | CITY-BETTER but PAID | SBK Punktmoln (2025: 16 pts/m2) and Höjdgrid 1 m (2021-2024) | 1 m grid; >= 16 pts/m2; height accuracy 9 cm hard surfaces | classified points + DTM grid | flown April 2025 | none in the records; page says cost per the city tariff | kartor.stockholm order | https://dataportalen.stockholm.se/dataportalen/srv/eng/csw |
| Helsinki | CITY-BETTER (open) | Helsingin korkeusmalli (WCS) | live WCS 1 m; Wayback dataset page says 50 cm; point cloud scans 2015/2017/2021 only seen in a snippet | elevation model | 2021 layer | WCS Fees NONE, AccessConstraints NONE; dataset licence not read | WCS | https://kartta.hel.fi/ws/geoserver/avoindata/wcs?service=WCS&request=GetCapabilities |
| Sofia | CITY-BETTER (modest) | Sofiaplan "Цифров модел на релефа" DTM and DSM with buildings | 5 m (from the file's world file) | DTM and DSM | 2017 | "Не са зададени лицензни права" | direct download, no login | https://urbandata.sofia.bg/api/3/action/package_show?id=digital-terrain-model-clean-surface-sofia-city |
| Warsaw, Kraków | NOTHING-BETTER than national (city-commissioned, published nationally) | GUGiK-published city lidar | point cloud >= 12 (Warsaw) / >= 10 (Kraków) pts/m2; NMT 1 m, NMPT 0.5 m | DTM and DSM | Kraków post 2023-12-11 | free download via geoportal.gov.pl | WFS/WCS/index | https://www.geoportal.gov.pl/aktualnosci/nowe-dane-fotogrametryczne-w-pzgik-10/ |
| Riga | UNVERIFIED | Riga 3D reality project 2023-24 lists a classified point cloud and DTM (55 km2) | not stated | | 2023-24 | only LOD2 models planned open | download not found | https://lvportals.lv/dienaskartiba/360234-izstradats-augstas-izskirtspejas-rigas-3d-realitates-un-3d-vektoru-modelis-2024 |
| Republika Srpska (RGURS) | RESTRICTED (confirmed) | "Digitalni model terena (2026)" 10 m to 20 cm | 5 resolutions, 20 local units | DTM | 2026 | capabilities: Fees none, AccessConstraints none, layer abstracts empty | anonymous GetMap on every layer tested returns "LayerNotDefined"; layers token-flagged | https://geoportal.rgurs.org/geoserver/dmt/wms?service=WMS&request=GetCapabilities |
| Federation of BiH | RESTRICTED (confirmed) | FGU/IPP lidar DTM 2018 (river corridors), DMR 5 m 2012, Sarajevo Canton 5 m 2007 | 1 m; 5 m | DTM | 2018; 2012; 2007 | "Nema ograničenja javnom uvidu"; use limited per regulations and tariffs; written request, fee | request | https://metapodaci.ippfbih.gov.ba/srv/eng/xml.metadata.get?uuid=2cfd6c83-748e-45ec-acc8-a9729e3173dc |
| Oslo, Copenhagen, Reykjavik, Tallinn, Ljubljana, Zagreb, Athens, Thessaloniki, Kyiv, Yerevan, Nicosia | NOTHING-BETTER | no city elevation model beyond the national one (Reykjavik: contour lines only; Copenhagen: 2013 3D city model; Zagreb: ZG3D buildings) | n/a | n/a | n/a | n/a | n/a | see notes 3.5 |
| Vilnius, Budapest, Bratislava, Belgrade, Sarajevo (city), Bucharest, Chişinău, Minsk, Ankara, Istanbul, Tbilisi, Baku, Moscow, St Petersburg, Brčko District, Tatarstan, Sakhalin, Kamchatka, Yakutia | UNVERIFIED (portals unreachable or blocked; no product seen) | none seen | n/a | n/a | n/a | n/a | n/a | see notes 3.5 and section 5 |

## 3. Notes per jurisdiction

Footprints and tile indexes are marked FOOTPRINT.

### 3.1 Nordics

Norway
- Kartverket: the whole country has a 1 m height model, delivered as point clouds (LAZ/ZLAS), DTM and DOM GeoTIFF, with WCS/WFS/WMS and REST services. https://www.kartverket.no/en/api-and-data/terrengdata
- Free products are CC BY 4.0, "©Kartverket". https://www.kartverket.no/en/api-and-data/terms-of-use
- Some hoydedata.no datasets are for Norge digitalt partners only; most lidar projects are open, some with access limits. https://kartkatalog.geonorge.no/api/getdata/f297e948-8a34-4e6c-9740-54b3a657f8d5
- Density at least 2 pts/m2 (5 in selected areas), project 2016-2022, goal a national 1 m DTM. https://kartkatalog.geonorge.no/api/search?text=nasjonal%20detaljert%20h%C3%B8ydemodell&limit=8
- DOM1 is made from all points minus noise classes 7 and 18 ("Bin with Maximum Value"). https://kartkatalog.geonorge.no/api/getdata/35e66707-942c-4aa2-9945-d769c2aeba35
- WCS for the 1 m terrain model (UTM33, fees "free"): https://wcs.geonorge.no/skwms1/wcs.hoyde-dtm-nhm-25833?service=WCS&request=GetCapabilities ; its record: https://kartkatalog.geonorge.no/api/getdata/0f0a0f38-00c4-4213-a9e5-2d861dc4abb0
- FOOTPRINT: WFS with project outlines (metadata_prosjekt:Prosjektavgrensning) and flight strips (Flystriper): https://wfs.geonorge.no/skwms1/wfs.hoyde-hoydedata-metadata-prosjekt?service=wfs&request=getcapabilities ; record https://kartkatalog.geonorge.no/api/getdata/18c447cb-24d0-4e38-b887-e279d66fbd0c (the agent found GetFeature as application/json unsupported). Download feed: https://nedlasting.geonorge.no/geonorge/Tjenestefeed.xml
- Old fallback DTM 10 (accuracy +-1 m in NDH areas, +-2 to 6 m elsewhere): https://kartkatalog.geonorge.no/api/search?text=digital%20terrengmodell%201%20meter&limit=8
- Municipal portals: not looked at individually; the Kartverket page showed none.

Svalbard and Jan Mayen
- Svalbard DTM 5: 5 m (2 m Bjørnøya), "covers parts of Svalbard", growing; CC BY 4.0. https://kartkatalog.geonorge.no/api/getdata/010bfb4a-65aa-4d30-aba9-7d090a432df7
- Svalbard DTM 20 covers the land area. https://kartkatalog.geonorge.no/api/getdata/9d118d31-182c-495b-b7be-d819cc7444b1
- Jan Mayen DTM 5 m, CC BY 4.0, maintenance "not planned". https://kartkatalog.geonorge.no/api/getdata/731a0602-d262-483d-aaa6-474cef68e82a
- No Svalbard lidar point cloud in the Geonorge catalogue. https://kartkatalog.geonorge.no/api/search?limit=15&text=Svalbard%20laser%20punktsky
- Fallback: ArcticDEM 2 m (DSM, ellipsoidal heights). https://www.pgc.umn.edu/data/arcticdem/ . FOOTPRINT: ArcticDEM mosaic index (https://data.pgc.umn.edu/elev/dem/setsm/ArcticDEM/indexes/ArcticDEM_Mosaic_Index_latest_gpkg.zip, HEAD 200, 1.0 MB), STAC (https://pgc-opendata-dems.s3.us-west-2.amazonaws.com/arcticdem/mosaics.json , https://stac.pgc.umn.edu/api/v1/), ImageServer (https://overlord.pgc.umn.edu/arcgis/rest/services/elevation/pgc_arcticdem_mosaics_latest/ImageServer?f=json). No extent file for the Svalbard DTM 5 found.

Sweden
- Lantmäteriet STAC: height data free, CC BY 4.0. https://api.lantmateriet.se/stac-hojd/v1/
- Collection dtm-cog: 1 m terrain height grid, COG, EPSG:5845, 2009-08 to 2026-07, CC-BY-4.0. https://api.lantmateriet.se/stac-hojd/v1/collections/dtm-cog . FOOTPRINT: its items list tile bounding boxes (10 km squares): https://api.lantmateriet.se/stac-hojd/v1/collections/dtm-cog/items
- Laserdata Skog (surface point cloud, laz/copc, 2019-10 to 2026-11) has separate terms. https://api.lantmateriet.se/stac-hojd/v1/collections ; https://www.lantmateriet.se/globalassets/geodata/geodataprodukter/anvandningsvillkor-for-laserdata-nedladdning-skog.pdf
- Geodataportalen record: "Rikstäckande", 1 m, TIN interpolation of ground/water points. https://www.geodata.se/geodataportalen/srv/api/records/1b48584d-f6d6-43fb-a6fd-7610a33de491/formatters/xml . The open-data page says CC0 for open geodata: https://www.lantmateriet.se/sv/geodata/vara-produkter/avgiftsfria-produkter/oppna-data/ ; Mapterhorn records CC0 too. Vertical accuracy: not found.

Finland
- 2 m DEM: from lidar of at least 0.5 pts/m2; "entire Finland" but not in parts of the outer archipelago and eastern border; class I about 0.3 m vertical; ~40 000 km2 added per year; CC BY 4.0. https://www.maanmittauslaitos.fi/en/maps-and-spatial-data/datasets-and-interfaces/product-descriptions/elevation-model-2-m . FOOTPRINT: status map https://tilannekartta.maanmittauslaitos.fi/km2 (app shell only).
- Open lidar 0.5 p: thinned from 5 p, CC BY 4.0, LAZ, only certain parts of Finland, standard error at most 10 cm. https://www.maanmittauslaitos.fi/en/maps-and-spatial-data/datasets-and-interfaces/product-descriptions/laser-scanning-data-05-p
- Lidar 5 p (all Finland by production area): strong identification and payment (EUR 0.026/km2, minimum 26.69). https://www.maanmittauslaitos.fi/en/maps-and-spatial-data/datasets-and-interfaces/product-descriptions/laser-scanning-data-5-p ; https://www.maanmittauslaitos.fi/laserkeilausaineistot ; https://www.maanmittauslaitos.fi/laserkeilausaineistot/kayttoehdot
- Programme: nine-year cycle, 20 pts/m2 from 2026. https://www.maanmittauslaitos.fi/laserkeilaus-ja-ilmakuvaus . FOOTPRINT (GeoPackage zips of the scanning plans): https://www.maanmittauslaitos.fi/sites/maanmittauslaitos.fi/files/attachments/2026/01/KALLIO2_keilausohjelma_5p.zip and https://www.maanmittauslaitos.fi/sites/maanmittauslaitos.fi/files/attachments/2026/01/KALLIO2_keilausohjelma_20p.zip
- API key (settled): the NLS page "Instructions for using the API key" says the key is needed for, among others, "Ortokuvien ja korkeusmallien kyselypalvelu (WCS)" and "Paikkatiedon tiedostopalvelu (OGC API Processes)"; "Open interface services are provided free of charge"; the key is a unique string created in NLS's My Account service after registering with an e-mail address. https://www.maanmittauslaitos.fi/en/rajapinnat/api-avaimen-ohje . The general page lists the access classes (open without key; open, free, login with an API key; agreement and fee): https://www.maanmittauslaitos.fi/en/maps-and-spatial-data/datasets-and-interfaces/general-information-about-interface-services . The 2 m product page says the model is "available from the NLS MapSite's Download geospatial data section" and names the file service and WCS URLs; the Finnish interface page lists the file service among NLS's open file packages: https://www.maanmittauslaitos.fi/rajapinnat/paikkatiedot . So the interface routes (WCS, file service) need a free key; whether the MapSite browser download needs sign-in was not tested.

Åland
- Government ArcGIS REST: layers copyright "LMV uträknade från höjdmodell 2m 2013 och 2015" (derived, no separate DEM). https://kartor.regeringen.ax/arcgis/rest/services?f=json ; https://kartor.regeringen.ax/arcgis/rest/services/Topografi/Hojd/MapServer?f=json . The open-data hub lists no elevation model: https://www.kartor.ax/api/search/v1/collections/all/items?limit=100

Denmark
- "0.4 m grid for the entire country". https://eng.kds.dk/green-transition-and-climate-adaptation/the-danish-elevation-model-dk-dem ; accuracy "a few centimetres to around half a metre": https://eng.kds.dk/mapping/aerial-photography-and-lidar-scanning
- Lidar 2014-10-01 to 2015-12-21, 4-5 pts/m2, 0.05 m vertical accuracy. https://cis2.eea.europa.eu/data/481 . A third party reports a 2025 version: https://help.emd.dk/mediawiki/index.php/Danish_Elevation_Model
- Free geodata are CC BY 4.0 since 2024-05-16. https://kds.dk/om-klimadatastyrelsen/vilkaar-og-priser . API token needs a Dataforsyningen user (not created). https://docs.dataforsyningen.dk/

Faroe Islands
- DTM 10 m (COWI photos 2009, ArcticDEM where missing), DSM 2/5/10 m made in 2015 from ArcticDEM; update "rare". https://www.foroyakort.fo/tak-datur-nidur/dtm-og-dsm-haeddarmodell/ ; licence via a third party: https://help.emd.dk/mediawiki/index.php/Faroe_Islands_Elevation_Models (Mapterhorn lists it as "Open Government Data"). FOOTPRINT-like ArcGIS REST: https://gis.us.fo/arcgis/rest/services?f=json ; https://gis.us.fo/arcgis/rest/services/dsm?f=json ; https://gis.us.fo/arcgis/rest/services/haeddarkurvar?f=json . No Faroese lidar found.

Iceland
- ÍslandsDEM v1.0, 2 m median mosaic of ArcticDEM, CC BY 4.0. https://gatt.natt.is/geonetwork/srv/api/records/e6712430-a63c-4ae5-9158-c89d16da6361/formatters/xml ; WMS https://gis.natt.is/mapcache/web-mercator/wms?request=GetCapabilities ; FTP listing (10/20/50 m) https://ftp.natt.is/gisdata/raster/ . No national lidar DTM found; the older LMÍ 2016 10 m mosaic is described at https://help.emd.dk/mediawiki/index.php/Iceland_Elevation_Model

Estonia
- Entire territory covered by ALS; LAZ 1.4 in 1 km2 files; DTM 1/5/10/25 m, DSM/nDSM 1/5 m; newest DEMs from 2021-2024; Open Data Licence. https://geoportaal.maaamet.ee/eng/spatial-data/elevation-data-p308.html ; download page (whole-country 5 m DTM 7.7 GB) https://geoportaal.maaamet.ee/eng/spatial-data/elevation-data/download-elevation-data-p664.html ; licence https://geoportaal.maaamet.ee/opendata-licence (one-page PDF dated 01.01.2025, read whole: free for an unspecified term, derivatives, commercial use and redistribution allowed, with credit to the Land and Spatial Development Board, the data title and its age, "as is"); heights EH2000 (EVRS, EPSG 5621), horizontal EPSG:3301, 2.5 m contours in the basic map. FOOTPRINT: the year-of-scan image (read) shows whole-country coverage flown in east-west and north-south blocks: 2008-2011 (west islands and north to south in bands), 2012-2015, 2017-2020, and the newest 2021-2024 as the west and Hiiumaa/Saaremaa (2021, 2024), the north-east (2022) and the south-east (2023) https://geoportaal.maaamet.ee/docs/pohikaart/lidar_aasta.jpg ; sheet indexes are zips of map-sheet grids, not scan footprints https://geoportaal.maaamet.ee/eng/spatial-data/map-sheet-indexes-and-coordinate-systems-p359.html . Lidar density is stated on none of the pages read.

Latvia
- LĢIA lidar: >= 4 pts/m2 (ground >= 1.5), classes ground/vegetation/buildings; open 20 m DTM. https://www.lgia.gov.lv/en/digital-height-models-0 ; https://www.lgia.gov.lv/en/Digit%C4%81lais%20virsmas%20modelis
- LĢIA's 2025 presentation: cycle 1 (2013-2019) is open data; cycle 2 (>= 6 pts/m2, 2022-2024, about 50% of the country, halted for funding) and the 1 m DTM/DSM/nDSM are free for public administration only. https://lgia.gov.lv/sites/lgia/files/document/Kartogr%C4%81fu_diena_2025_pakalpojumi.pdf
- NON-GOVERNMENTAL: rapidlasso reports 65 939 LAS tiles over the whole country, CC0 1.0 (2019 post). https://rapidlasso.de/national-open-lidar-strategy-of-latvia-humiliates-germany-austria-and-other-european-closed-data-states/ . Mapterhorn uses LĢIA's 20 m DTM (CC BY 4.0, section 4).

Lithuania
- The geoportal.lt metadata catalogue (CSW, 800 records; I paged all titles) holds NŽT "Lidar_DR_LT - skaitmeniniai erdviniai lazerinio skenavimo taškų duomenys" records for 2019, 2020, 2021, 2022, 2025 and 2026 plus a rolling "Aktualiausi" (2019-2026) record. The rolling record: density "ne mažiau 6.5 tšk./m2", RMSE vertical <= 10 cm and horizontal <= 30 cm, 8 LAS classes (2 = ground), automatic classification, LAZ in LKS-94 sheets, "Each sheet contains a URL for downloading", bbox 20.5972-27.3822 E, 53.7639-56.7073 N (the whole country), metadata date 2026-08-11; constraints are only the words "copyright" and "license". https://www.geoportal.lt/metadata-catalog/csw?service=CSW&version=2.0.2&request=GetRecordById&elementSetName=full&outputSchema=http://www.isotc211.org/2005/gmd&id=%7B3C0D5E47-49CA-4132-88C1-EC3D1E4610B4%7D . The 2026 record: nominal density >= 15 pts/m2, RIEGL scanners, same accuracy, metadata 2026-08-12. https://www.geoportal.lt/metadata-catalog/csw?service=CSW&version=2.0.2&request=GetRecordById&elementSetName=full&outputSchema=http://www.isotc211.org/2005/gmd&id=%7BFEB7F0A6-3BD4-4BF9-AE93-4A4380904E6D%7D . Catalogue root: https://www.geoportal.lt/metadata-catalog/csw?service=CSW&version=2.0.2&request=GetCapabilities
- FOOTPRINT: coverage images named in the record (HEAD 200): https://www.geoportal.lt/download/Matadata/Images/NZT/lidar/Lidar_DR_LT.png and https://www.geoportal.lt/download/Matadata/Images/NZT/lidar/Lidar_DR_LT_recent.png
- Also in the catalogue (titles only): "Skaitmeninis upių slėnių aukščių modelis" (river-valley DSM, Environmental Protection Agency, lidar-based, 2014): https://www.geoportal.lt/metadata-catalog/csw?service=CSW&version=2.0.2&request=GetRecordById&elementSetName=full&outputSchema=http://www.isotc211.org/2005/gmd&id=%7B64A10467-1F79-64C2-7C01-8511AA1F234D%7D ; GDR50LT relief model, county-centre points (SEŽP/SEOP), and SEŽP_0.5LT (listed "Not provided" in INSPIRE).
- Access and licence not settled: the earlier-read INSPIRE table says the national set is "Not provided"; the download-services page requires a login; the open-data page lists only EuroDEM 1:100 000 (2008). So Lithuania has a national lidar (6.5-15 pts/m2) whose open/free status I could not confirm. Routes tried for the terms: geoportal.lt open-data, INSPIRE, download-services pages (https://www.geoportal.lt/geoportal/web/inspire-en/data-availability , https://www.geoportal.lt/geoportal/web/en/open-data , https://www.geoportal.lt/geoportal/web/en/download-services), the CSW records above (no licence text), data.gov.lt API (HTTP 500 on three paths, and the Wayback availability API had no snapshot: https://archive.org/wayback/available?url=data.gov.lt/api/3/action/package_search?q=lidar), the Wayback copy of the NŽT open-data page (no lidar named: https://web.archive.org/web/20241125215406/https://nzt.lrv.lt/lt/atviri-duomenys/). Fallback GLO-30.

### 3.2 Central Europe and Balkans

Poland
- NMT: 1 m grid updated from ALS and (cities) stereo photogrammetry; 5 m grid from 25 cm imagery; free, "dowolne wykorzystanie". https://www.geoportal.gov.pl/pl/dane/numeryczny-model-terenu-nmt/
- ALS covers the country at 4-20 pts/m2; LAS/LAZ free, class 2 ground. https://www.geoportal.gov.pl/pl/dane/dane-pomiarowe-lidar-lidar/ . NMPT (DSM): https://www.geoportal.gov.pl/pl/dane/numeryczny-model-pokrycia-terenu-nmpt/
- FOOTPRINT: WFS sheet indexes SkorowidzNMT2018 to 2024 (Wayback capabilities copy read; live host timed out): https://web.archive.org/web/2024/https://mapy.geoportal.gov.pl/wss/service/PZGIK/NumerycznyModelTerenuEVRF2007/WFS/Skorowidze?service=WFS&request=GetCapabilities . Other index endpoints named on the NMT page (not fetched live): https://mapy.geoportal.gov.pl/wss/service/PZGIK/NumerycznyModelTerenuKRON86/WFS/Skorowidze and https://mapy.geoportal.gov.pl/wss/service/PZGIK/NMT/WMS/SkorowidzeUkladEVRF2007
- Vertical accuracy not stated on the pages read. Mapterhorn lists Poland 5 m country-wide and 1 m partial (section 4). Voivodeship portals: none found publishing own better data, not exhaustively checked.

Czechia
- DMR 5G: TIN from ALS 2009-2013, whole country finished 2016-06-30; mean error 0.18 m open / 0.3 m forest; LAZ per SM5 sheet, CC BY 4.0; WMS, Atom, clip export. https://geoportal.cuzk.gov.cz/(S(1))/Default.aspx?mode=TextMeta&side=dmr5g&metadataID=CZ-CUZK-DMR5G-V&menu=302
- DMR 4G: 5 m grid, error 0.3 m open / 1 m forest. https://geoportal.cuzk.gov.cz/(S(1))/Default.aspx?mode=TextMeta&side=dmr5g&metadataID=CZ-CUZK-DMR4G-V&menu=302
- DMP 1G (DSM, error 0.4 m buildings, 0.7 m vegetation) will not be provided after 2027-01-01; replaced by DMP OK. https://geoportal.cuzk.gov.cz/(S(1))/Default.aspx?mode=TextMeta&side=dmr5g&metadataID=CZ-CUZK-DMP1G-V&menu=302 . Product list: https://geoportal.cuzk.gov.cz/(S(1))/Default.aspx?mode=TextMeta&side=vyskopis&menu=3 ; ATOM front page https://atom.cuzk.gov.cz/
- FOOTPRINT candidates: https://ags.cuzk.gov.cz/arcgis/rest/services/KladyMapovychListu/MapServer?f=pjson and https://ags.cuzk.gov.cz/arcgis/rest/services?f=pjson (sheet grids; not confirmed to hold the SM5 grid).
- Prague: IPR Praha DTM, bare-ground raster from lidar at about 40 pts/m2, CC BY. https://www.arcgis.com/sharing/rest/content/items/97d2c9c11aa9478cb21b469b8a4f820e?f=json ; credit "datový podklad © IPR Praha": https://geoportalpraha.cz/data-a-sluzby/otevrena-data . Brno has a DMT 2019 catalogue page (https://gis.brno.cz/ost/edas/public/7517ffb8-6adc-4346-b87b-f37ffdc27f16) that did not render; its content is unverified.

Slovakia
- Cycle 1 (2017-2023, 42 lots): >= 5 pts/m2, point-cloud abs. height accuracy <= 0.15 m, DMR 5.0 <= 0.20 m; cycle 2 (2022-2026, 73 lots): >= 15 pts/m2, <= 0.10 m. https://www.skgeodesy.sk/geoportal/zbgis/lls/ ; https://www.skgeodesy.sk/geoportal/zbgis/lls/1-cyklus/
- Access: cycle 1 DMR 5.0 / DMP 1.0 at 1 m as country ZIPs (DMR 183 GB), cycle 2 DMR 6.0 / DMP 2.0 at 0.5 m by lot (16 lots listed available), MAPKA clips to 400 km2, LAZ on order at counters; CC-BY 4.0 "ÚGKK SR". https://www.skgeodesy.sk/geoportal/zbgis/lls/poskytovanie/
- FOOTPRINT: LAS footprint ZIPs (cycle 2 ETRS89 GDB HEAD 200, 2.7 MB): https://opendata.skgeodesy.sk/static/LLS/Footprints/2_cyklus/las_footprints_etrs89-tm34_2_cyklus_gdb.zip ; cycle 1 siblings in https://opendata.skgeodesy.sk/static/LLS/Footprints/1_cyklus/ (named on the page).
- Bratislava: no separate city product found.

Hungary
- Lechner DDM5: 5 m grid from 1:10 000 contours (1979-2000) with stereo photogrammetry, mean height reliability +-0.7 m; national refreshes 2000/2005/2015/2022; prices per km2 (5 m 1920 Ft, 10 m 1440 Ft, 25 m free) plus 3000 Ft cutting fee. https://lechnerkozpont.hu/node/1963 . No open lidar DTM found; counties not checked.

Slovenia
- CLSS 2023-25: laser 10 pts/m2 with 16 cm photos; products GKOT, DMR, DMP, nDMP; LAZ 1.4 and GeoTIFF; "No limitations" per the GURS record (Wayback; live host timed out). https://web.archive.org/web/2026/https://eprostor.gov.si/imps/srv/api/records/951eaa88-0f6b-4d29-8d02-eaf56534453e/formatters/xml . Viewer/download portal (JS): https://clss.si/
- GURS DMV 5 m (2011-2014), accuracy 1 m open / 3 m overgrown, free: https://www.e-prostor.gov.si/podrocja/drzavni-topografski-sistem/digitalni-modeli-visin/ ; CKAN CC BY 4.0: https://podatki.gov.si/api/3/action/package_show?id=digitalni-model-visin
- NON-GOVERNMENTAL vendor pages (not authoritative): LSS 2011-2015 DMR1 1 m, 1 km2 sheets, 0.1 m estimated height accuracy. https://www.kingprostor.si/blog/arso-lidar/ ; https://site.geo-portal.si/2021-01-04-vkljucitev-lidar-2.html . The official ARSO LiDAR page answered "temporarily unavailable". https://gis.arso.gov.si/evode/profile.aspx?id=atlas_voda_Lidar%40Arso

Croatia
- DGU DMR page: lidar >= 8 pts/m2, DMV about 1 m from lidar; lidar 2022-2023, photogrammetry 2023-2024; fees; written request. https://dgu.gov.hr/proizvodi-i-usluge/podaci-topografske-izmjere/digitalni-model-reljefa/180
- CONFLICT, quoted. (a) DGU product page for the relief model (DMR/DMV): "Cijena korištenja podataka utvrđena je prema Pravilniku o određivanju visine stvarnih troškova izdavanja i uvjeta korištenja podataka (53/23, 106/25)" and "Zahtjev se predaje u pisarnici, šalje poštom, email-om ili faxom"; "godina zadnjeg ažuriranja: aerofotogrametrijsko snimanje 2023.- 2024., LiDAR snimanje (2022. - 2023.)". https://dgu.gov.hr/proizvodi-i-usluge/podaci-topografske-izmjere/digitalni-model-reljefa/180 . (b) NIPP registry source 1694, "Multisenzorsko zračno snimanje Republike Hrvatske - LiDAR podaci" (classified source laser data, 10 classes incl. 2 = teren, format las/laz, series): "Ograničenje pristupa i korištenja izvora : Nema uvjeta za pristup i korištenje", "Datum uključivanja izvora u NIPP : 2025-04-16", "Adresa izvora" = the DGU project page. https://registri.nipp.hr/izvori/1871 . Which is newer: neither page prints its own edit date. The NIPP record gives only its inclusion date, 2025-04-16. The DGU page cites the fee regulation as "53/23, 106/25", an amendment from 2025 (issue 106/25 of Narodne novine; I did not fetch its date), so the DGU text was edited at least once in 2025 and is probably the later page, but that is an inference. They also describe different products: the NIPP entry the raw classified point cloud, the DGU page the DMR/DMV products. The project page that the NIPP record points to (https://dgu.gov.hr/multisenzorsko-zracno-snimanje-republike-hrvatske/5700) shows no download link in the text I read. Net: a download of the point cloud was not found; treat as RESTRICTED until a download is seen.
- Project: two lots (50.5% east/south, 49.5% west) plus levees. https://dgu.gov.hr/multisenzorsko-zracno-snimanje-republike-hrvatske/5700 . nDSM 1 m (LIFE CROLIS) on request: https://www.lifecrolis.hr/en/2024/10/18/the-normalized-digital-surface-model-of-croatia-is-available-for-use/ . Spec PDF (HEAD 200, 1.2 MB, not read): https://dgu.gov.hr/UserDocsImages/dokumenti/Istaknute%20teme/Multisenzorno%20snimanje/LiDAR%20snimanje%20iz%20zraka.pdf . City portals (Zagreb) not checked.

Bosnia and Herzegovina (checked per unit)
- Re-check (confirmation pass): FBiH GeoNetwork search for "lidar" returns two records (the 2018 lidar DTM and the 2018 10 cm orthophoto), both "Nema ograničenja javnom uvidu" with use "in accordance with valid regulations and tariffs"; the cadastre page https://katastar.ba/koristenje says a written request goes to the competent authority, a fee is paid per the tariff and the documentation may be used only for the purpose issued. Other FBiH DTMs have the same terms: 5 m (2012), 25 m (2003), 50 m (2008) and a Sarajevo Canton 5 m DTM from contours (2007). https://metapodaci.ippfbih.gov.ba/srv/eng/q?_content_type=json&fast=index&any=lidar ; https://metapodaci.ippfbih.gov.ba/srv/eng/xml.metadata.get?uuid=2cfd6c83-748e-45ec-acc8-a9729e3173dc ; https://metapodaci.ippfbih.gov.ba/srv/eng/xml.metadata.get?uuid=e555a373-eb44-4582-a93d-2557ca4255d9 ; https://metapodaci.ippfbih.gov.ba/srv/eng/xml.metadata.get?uuid=097737b8-a0cd-4e7a-ba25-2f019aeab8fc ; https://metapodaci.ippfbih.gov.ba/srv/eng/xml.metadata.get?uuid=c01a42cf-a851-4d2d-8424-25646045621f ; https://metapodaci.ippfbih.gov.ba/srv/eng/xml.metadata.get?uuid=adbbd0cd-a68a-43cc-b999-d38d7ace03d4 ; https://metapodaci.ippfbih.gov.ba/srv/eng/xml.metadata.get?uuid=f28e113e-d54b-4b78-9f2b-c49b5c2115e9
- RGURS re-check: the WMS capabilities say service-level `<Fees>none</Fees>` and `<AccessConstraints>none</AccessConstraints>` with empty layer abstracts, but anonymous 64x64 GetMap requests on dmt_10m, dmt_1m, dem:dem and others returned "LayerNotDefined / Could not find layer" (the 2012 "Visine" group is token-flagged too), so the layers cannot be fetched without a token. https://geoportal.rgurs.org/geoserver/dmt/wms?service=WMS&request=GetCapabilities ; https://geoportal.rgurs.org/geoserver/dem/wms?service=WMS&request=GetCapabilities
- The EU IPA II-2019 nationwide lidar (FGU page, Wayback): all of BiH, point clouds, DTM and DSM at 5 and 10 pts/m2 (urban, rural), planned 2023-2024; FBiH, RS and Brčko named as users; delivery and licence not stated. https://web.archive.org/web/2025/https://fgu.com.ba/en/the-surveying-project-of-bosnia-and-herzegovina-territory-lidar-ipa-ii-2019.html . Brčko: its government site (Wayback) shows only a menu and the Public Register head page, nothing on elevation. https://web.archive.org/web/20260107004209/https://vlada.bdcentral.net/ ; https://web.archive.org/web/2026/https://vlada.bdcentral.net/Content/Read/sef-odjeljenja-za-javni-registar
- Federation of BiH (FGU/IPP): lidar DTM 2018, 1 m, 4-20 pts/m2, abs height accuracy 10 cm, TIFF/ASCII/LAS 1.4, river corridors (3288 km2 across BiH per the record); public viewing unrestricted, use limited per regulations and tariffs. https://metapodaci.ippfbih.gov.ba/srv/api/records/2cfd6c83-748e-45ec-acc8-a9729e3173dc . National DMR 5 m (2012): https://metapodaci.ippfbih.gov.ba/srv/api/records/adbbd0cd-a68a-43cc-b999-d38d7ace03d4 ; agency site https://katastar.ba/
- Republika Srpska: geoportal catalogue lists "Digitalni model terena (2026)" for 20 local-government units at 10 m, 5 m, 1 m, 50 cm, 20 cm, flagged `keycloakToken: true`. https://geoportal.rgurs.org/init/geoportal.json ; https://geoportal.rgurs.org/config.json . FOOTPRINT-like: WMS capabilities (one RS-wide box, not per-unit): https://geoportal.rgurs.org/geoserver/dmt/wms?service=WMS&request=GetCapabilities . Landing page has no elevation page: https://www.rgurs.org/ . Layers render as hillshade; I did not log in.
- Brčko District: no public product found; the government site did not respond.
- Nationwide EU-funded lidar (about EUR 1.8 M): all 44 blocks flown, data delivered to FBiH FGU, RS RUGIPP and the Brčko Public Register, processing ongoing (news article, not the publisher). https://www.klix.ba/scitech/nauka/bosna-i-hercegovina-uskoro-dobija-prvu-jedinstvenu-3d-kartu-zemlje-zavrseno-lidar-snimanje/260825121

Serbia
- RGZ DMT: grids 5, 10, 25 m with breaklines, 2011-2013 update of 2007-2010; lidar of the Tisa floodplain (2012, about 2920 km2, 1 m, mean height accuracy 0.07 m); lidar for 74 flood areas (about 9427 km2) in progress; issue under the administrative-fees law. https://www.rgz.gov.rs/digitalni-model-terena . No Vojvodina or Belgrade product found.

Montenegro, Kosovo
- Montenegro: only a 2010 trade report of a laser DTM contract (Geofoto, Norwegian-funded), no evidence of publication. https://www.gim-international.com/content/news/dtp-and-dop-for-entire-montenegro
- Kosovo: KCA site and geoportal show no elevation product in what was read. https://www.kca-ks.org/ ; https://geoportal.rks-gov.net/

North Macedonia
- AKN lidar: MGGP Aero under a contract with AKN and Norwegian Kartverket; point cloud, DTM, DSM for 11 072 km2 in 14 blocks; products "will be public" through the LiDAR portal (Wayback). https://web.archive.org/web/2026/https://aknlidarappprod.katastar.gov.mk/LiDARInfoStrana/index.html ; AKN home lists lidar data as a service: https://web.archive.org/web/2026/https://www.katastar.gov.mk/en/home/

Albania
- ASIG: lidar for DTM/DSM at 5 pts/m2 (urban, ~1028 km2), 2 pts/m2 (rural, ~10 340 km2), up to 1 pt/m2 (mountains, ~17 380 km2); "83% of the territory covered by aerial photography" (photography, not elevation). https://asig.gov.al/en/aerial-photography-in-the-republic-of-albania/ . The geoportal news item says the 2015-17 DSM was published: https://geoportal.asig.gov.al/en/node/954 . No download terms found.

### 3.3 SE Europe, Turkey, Caucasus

Romania
- ANCPI MNT licence (OGL-ROU-1.0, attribution "Date realizate de ANCPI", EPSG:3844, not usable as court evidence); built from 1:50 000 contours (updated 2014-15 and 2019-20) with component DTMs at 1, 2 and 5 m. https://help.emd.dk/mediawiki/images/c/ca/Licen%C8%9Ba_deschisa_Modelul_numeric_al_terenului_v2_MNT.pdf . Secondary description (ANCPI-stated accuracy 10 m mountains, 5 m hills, 1.5 m flat; 5 and 20 m resamples): https://help.emd.dk/mediawiki/index.php/Romania_Elevation_Model
- Lidar: LAKI II finished 2022 at 8 pts/m2; LAKI III under way at 5 pts/m2; airborne data above 9 pts/m2 showing sensitive content is a state secret (Law 182/2002 as amended by 167/2015). https://lidarmag.com/2024/09/22/elevations-for-the-nations-part-ii/ ; https://lidarmag.com/2020/11/01/lidar-initiatives-at-the-national-center-of-cartography-romania/
- Mapterhorn republishes "LAKI II MNT 0.5m" from geoportal.gov.ro under OGL-ROU-1.0 (35 GiB raw): https://download.mapterhorn.com/attribution.json . No county or city open lidar found. The national geoportals did not answer.
- Newer lead (preprint, 2026-06-07): public 0.5 m LAKI III Zone A DTM for four counties (Caraş-Severin, Gorj, Mehedinţi, Dolj), Zone B next; not checked at the publisher. https://arxiv.org/html/2606.08876v1 . Bucharest: no municipal product found (geoportal.pmb.ro, opendata.pmb.ro, data.bucuresti.ro did not answer).

Bulgaria
- Ministry of Agriculture INSPIRE view service (WMS 1.3.0), metadata 2024-05-27, access limits under Art. 13(1)(d). https://web.archive.org/web/2025/https://inspire-geoportal.ec.europa.eu/srv/api/records/a03ad76a-b38e-4aea-b95e-264ebad4f090/formatters/xml . FOOTPRINT-like: live capabilities list Dem_2020 to Dem_2025 (+Dem_Alternative) with per-layer bounding boxes (rectangles); DTM for 2023-25, DSM for 2020-22. http://inspire.mzh.government.bg:8080/geoserver/ows?service=wms&version=1.3.0&request=GetCapabilities . Cadastre site shows no elevation product: https://www.cadastre.bg/

Greece
- Hellenic Cadastre INSPIRE page: DTMs must be applied for (live page 403; Wayback read). https://web.archive.org/web/2025/https://www.gov.gr/ipiresies/periousia-kai-phorologia/ktematographese/geopule-geokhorikon-dedomenon-inspire-ellenikou-ktematologiou
- LIDAR Magazine (Aug 2025): planned 1 m DSM and DTM, south first, north after; no national lidar coverage reported yet. https://lidarmag.com/2025/08/30/elevations-for-the-nations-part-iii/ . Open-data news mentions no elevation product: https://www.taxheaven.gr/news/57349/kthmatologio-prosbash-se-oloys-sta-anoixta-dedomena-mesw-toy-dataktimatologiogr

Cyprus
- DLS e-service: orthophotos, DTM, DSM, contours of "different time periods", free up to 1 GB/day per registered user, otherwise EUR 17/km2 + VAT for DTM/DSM. https://portal.dls.moi.gov.cy/en/?p=18685 ; data page (contours and hydrography free): https://portal.dls.moi.gov.cy/en/application_forms/diathesi-geografikon-psifiakon-dedomenon/ ; section page: https://portal.dls.moi.gov.cy/en/to-tmima/kladoi/geodaisias-ydografias-fotogrammetrias/tomeas-fotogrammetrias-kai-tilepiskopisis-2/
- Mapterhorn republishes "Cyprus DTM 2019" from the Department of Lands and Surveys under CC BY 4.0, 1 m, country-wide: https://mapterhorn.com/ ; https://download.mapterhorn.com/attribution.json . Lidar density (4 pts/m2, 2014) appeared only in sources that did not load.
- Northern Cyprus: TKGM (Turkey) tendered true orthophoto and 3D work in 2025; no data release. https://sabah.com.tr/ekonomi/kktcnin-dijital-ikizi-cikarilacak-7367458 ; tender text https://www.tkgm.gov.tr/sites/default/files/2025-06/IlanMetni_Kibris_3B.pdf (PDF not readable here). SBAs: nothing found.

Turkey
- HGM "Yükseklik Verileri (Grid)": SYM12 and SAM12 free for public bodies only, DTED-2 8873 TL, SAM 30 m 872 TL, SYM5-L0 144 TL; order by form to HGM (Wayback). https://web.archive.org/web/2025/https://www.harita.gov.tr/urunler/yukseklik-verileri-grid/1 . SYM12: DSM from radar data 2011-2014, RMSE about 2.3 m: https://dergipark.org.tr/en/pub/tuzal/issue/92575/1665238 . TUCBS lists no public lidar: https://tucbs.gov.tr/

Georgia, Armenia, Azerbaijan
- Georgia: NAPR page lists NSDI and a geoportal, no elevation product. https://napr.gov.ge/ ; https://nsdi.gov.ge/ (script shell).
- Armenia: Cadastre Committee did 2014 aerial photography with a digital relief model and orthophotos, no resolution or access stated. https://cadastre.am/en/cadastre_mapping . Institute of Geological Sciences lists DEMs at 1:100 000, 1:25 000, and 1:10 000 for Yerevan, access not stated. https://geology.am/en/Lab-geoinformatics-scientific-achievements/
- Azerbaijan: nothing found.
- GLO-30 covers Armenia, Azerbaijan and Moldova since the Dec 2023 release. https://dataspace.copernicus.eu/explore-data/data-collections/copernicus-contributing-missions/collections-description/COP-DEM (also https://dataspace.copernicus.eu/node/1306)

### 3.4 Ukraine, Belarus, Moldova, Russia

Ukraine
- The national open-data portal returns no datasets for relief, lidar, or relief model. https://data.gov.ua/api/3/action/package_search?q=DEM&rows=5 (same endpoint for the Ukrainian queries). The geoportal nsdi.gov.ua is a login page: https://nsdi.gov.ua/ ; land.gov.ua mentions no relief data: https://land.gov.ua/ . GLO-30 tile over Kyiv exists (HEAD 200, 28.9 MB): https://copernicus-dem-30m.s3.amazonaws.com/Copernicus_DSM_COG_10_N50_00_E030_00_DEM/Copernicus_DSM_COG_10_N50_00_E030_00_DEM.tif ; Crimea tile: https://copernicus-dem-30m.s3.amazonaws.com/Copernicus_DSM_COG_10_N46_00_E035_00_DEM/Copernicus_DSM_COG_10_N46_00_E035_00_DEM.tif . City portals (Kyiv, Lviv) not checked.

Belarus
- The cadastre agency's 2019 UN-GGIM slides list aerial survey with orthophoto (30 cm GSD, 5-year cycle), no DEM. https://ggim.un.org/meetings/2019/IWS/Minsk/documents/S2-7%20Belarus-Marina%20Litreeva.pdf . A BSU paper used its own research lidar, not a national product. https://elib.bsu.by/bitstream/123456789/308729/1/128-134.pdf . GLO-30 Minsk tile exists: https://copernicus-dem-30m.s3.amazonaws.com/Copernicus_DSM_COG_10_N53_00_E027_00_DEM/Copernicus_DSM_COG_10_N53_00_E027_00_DEM.tif

Moldova
- INSPIRE record (Wayback): DTM 2007 except the left bank, made from an aerial survey; owner AGCC, author INGEOCAD; "noLimitations" / "noConditionsApply". https://web.archive.org/web/20260510121956/https://inspire-geoportal.ec.europa.eu/srv/api/records/38dac355-7cc1-4110-ab04-7406e3e5fbb5
- WMS lists DTM Moldova 2007 (5 m), DTM_2020_centru and DTM_2021_Sud (per-layer boxes; FOOTPRINT-like). https://geodata.gov.md/geoserver/ows?service=WMS&request=GetCapabilities . WCS and WFS answer "disabled": https://geodata.gov.md/geoserver/ows?service=WCS&request=GetCapabilities ; https://geodata.gov.md/geoserver/ows?service=WFS&request=GetCapabilities . A community catalogue says the same: https://source.coop/nlebovits/moldova-geodata/wms/dtm/AGENTS.md . GLO-30 Moldova tile: https://copernicus-dem-30m.s3.amazonaws.com/Copernicus_DSM_COG_10_N47_00_E028_00_DEM/Copernicus_DSM_COG_10_N47_00_E028_00_DEM.tif
- Newer lead (news, 2026-04-20): the national cadastre agency is running a nationwide aerial photogrammetric survey (25 million lei) with a special flight over Chişinău; products are digital relief models, orthophotos and 3D stereo collection, to be free online. Photogrammetry, not stated as lidar; nothing published yet that I could see. https://capital.ro/oglinda-digitala-a-republicii-moldova-teritoriul-va-fi-scanat-3d-din-avion-pentru-harti-gratuite-de-inalta-precizie.html

Russia
- The federal fund (FFPD) supplies data on application, with a contract and a fee (free for state and municipal bodies); the centre's home page lists "digital relief model" among its photogrammetry work, but no grid, coverage or licence is stated (Wayback). https://web.archive.org/web/20241206110844/https://cgkipd.ru/fsdf/service/provide/ ; https://web.archive.org/web/20241206110844/https://cgkipd.ru/ ; https://web.archive.org/web/20241206110844/https://cgkipd.ru/fsdf/vedenie-ffpd/dostupnost/ . A statutory ban on elevation data was not found (not a legal claim).
- Federal subjects: Wayback home pages of Tatarstan's FPD, Moscow open data, St Petersburg RGIS, Moscow oblast RGIS, Krasnodar open data and data.gov.ru showed no elevation product (home pages only, little text). https://web.archive.org/web/20260106145652/https://fpd.tatarstan.ru/ ; https://web.archive.org/web/20260120160200/https://data.mos.ru/ ; https://web.archive.org/web/20250812001052/https://rgis.spb.ru/ ; https://web.archive.org/web/20250130125200/https://rgis.mosreg.ru/v3/ ; https://web.archive.org/web/20220122143206/https://opendata.krasnodar.ru/ ; https://web.archive.org/web/20251229012941/https://data.gov.ru/ . Sakhalin, Kamchatka, Yakutia: not read.
- ArcticDEM (north): 2 m DSM strips and mosaics, WGS84 ellipsoid heights, no ground control; the page does not list Russia's coverage. https://www.pgc.umn.edu/data/arcticdem/ ; licence CC-BY-4.0 in the STAC collection https://pgc-opendata-dems.s3.us-west-2.amazonaws.com/arcticdem/mosaics.json . GLO-30 tiles over Moscow (https://copernicus-dem-30m.s3.amazonaws.com/Copernicus_DSM_COG_10_N55_00_E037_00_DEM/Copernicus_DSM_COG_10_N55_00_E037_00_DEM.tif) and Murmansk (https://copernicus-dem-30m.s3.amazonaws.com/Copernicus_DSM_COG_10_N69_00_E033_00_DEM/Copernicus_DSM_COG_10_N69_00_E033_00_DEM.tif) exist.

### 3.5 Cities and regional bodies

Brno (CITY-BETTER)
- The catalogue page is a single-page app; its JSON is at getDetailData.php. DMT 2019: published by the city's GIS department (Magistrát města Brna, Odbor městské informatiky), licence_name "CC BY" (deed 4.0 link), served as ArcGIS ImageServers and downloads (TIFF, XYZ, ShapeFile, DWG, DGN); models listed: DMT 2019, DMR 4G, DMR 5G, DMT JMK 2012 and older. https://gis.brno.cz/ost/edas/public/scripts/getDetailData.php?uuid=7517ffb8-6adc-4346-b87b-f37ffdc27f16 ; full catalogue https://gis.brno.cz/ost/edas/public/scripts/getCatalogData.php
- Grid: ImageServer pixelSizeX/Y 0.25 m, F32 elevation, EPSG:5514; FOOTPRINT: the same answer's extent (xmin -611431.8, ymin -1170860.97, xmax -588490.55, ymax -1149314.47), about 22.9 x 21.5 km. https://gis.brno.cz/ags1/rest/services/OMI/OMI_DMT_2019_elevation/ImageServer?f=json . Lidar density not stated (documentation is in a SharePoint folder, not opened).
- The yearly DMP (surface model, 2019 and 2022-2026) is made by image autocorrelation from spring aerial photos (10 cm, generalised to 25 cm), CC BY; contours 2019 at 1 m are derived from the DMT. https://gis.brno.cz/ost/edas/public/scripts/getDetailData.php?uuid=33339dd5-bdd2-4e47-8fb4-5b5562e3d296 ; https://gis.brno.cz/ost/edas/public/scripts/getDetailData.php?uuid=d263c3ad-28d2-4db4-a07f-49651e1fd266

Gothenburg (CITY-BETTER)
- "Höjdmodell 2022": GeoTIFF Float32, 0.5 m per pixel, 1x1 km tiles in 10x10 km zips, whole municipality, SWEREF 99 12 00, RH 2000; owner Göteborgs Stad; "Licens: Creative Commons, CC0"; airborne laser scanning on 9, 15 and 16 May 2022, 10 pts/m2. The sheet does not say DTM or DSM. https://opengeodata.goteborg.se/files/hojdmodell/2022/hojdmodell_2022_sv.pdf ; English sheet (HEAD 200) https://opengeodata.goteborg.se/files/hojdmodell/2022/hojdmodell_2022_en.pdf . Zip downloads sit behind the city's open-data page (the individual zip URLs were not found; the directory listing returned 500): https://goteborg.se/psidata . Press release: https://www.mynewsdesk.com/se/goteborgsstad/pressreleases/stadsbyggnadskontoret-i-goeteborg-slaepper-fler-oeppna-grundlaeggande-geodata-nu-med-projekteringsfokus-3016588

Stockholm (CITY-BETTER, but paid)
- Stockholms stad Stadsbyggnadskontoret via its GeoNetwork CSW: "SBK Punktmoln - flygburen laserskanning" 2023, 2024, 2025 (2025: flown 1-3 April 2025, 16 pts/m2, height accuracy 9 cm hard surfaces / 15 cm elsewhere, classes 1/2/6/7/9/17, metadata 2026-02-03) and "SBK Höjdgrid" 2021-2024 (1 m grid interpolated from the laser data). Footprint of the 2023 grid record: lon 17.7605-18.2011, lat 59.2272-59.4402. https://dataportalen.stockholm.se/dataportalen/srv/eng/csw
- Terms: the records carry no licence; the order page says "Kostnaden beräknas enligt stadsbyggnadsnämndens taxa" for both products, and that some WMS/vector products are free. https://kartor.stockholm/kartor-geodata/ . Test file (246 MB, read whole): LAS 1.2, point format 2, 9,461,888 points over a 851 x 762 m block (0.65 km2, about 14.6 pts/m2 on average), heights -3.1 to 100.4 m, horizontal EPSG:3011 (SWEREF 99 18 00), no vertical system in the header: https://dataportalen.stockholm.se/dataportalen/Data/Stadsbyggnadskontoret/Testdata_Punktmoln_2025.las

Helsinki (CITY-BETTER, open)
- The live WCS lists only "Korkeusmalli_2021_1m"; Fees NONE, AccessConstraints NONE; DescribeCoverage gives a 1.0 m offsetVector, EPSG:3879, extent 6655000-6690000 N, 25487000-25516000 E (FOOTPRINT). https://kartta.hel.fi/ws/geoserver/avoindata/wcs?service=WCS&request=GetCapabilities ; https://kartta.hel.fi/ws/geoserver/avoindata/wcs?service=WCS&version=2.0.1&request=DescribeCoverage&coverageId=avoindata__Korkeusmalli_2021_1m
- The Wayback copy (2024-10-05) of the catalogue entry says "The resolution is 50 cm / pixel", layer "Korkeusmalli_2021_50cm", height system N2000, last modified 2024-04-06; the 50 cm vs 1 m difference is unresolved. https://web.archive.org/web/2024/https://avoindata.fi/data/en/dataset/helsingin-korkeusmalli . A 0.5 m hillshade WMS layer is open: https://kartta.hel.fi/ws/geoserver/avoindata/wms?service=WMS&request=GetCapabilities . The CC BY 4.0 statement found is for the 3D city model, not the DEM: https://www.hel.fi/en/decision-making/information-on-helsinki/maps-and-geospatial-data/helsinki-3d . The city's LAZ point cloud (2015/2017/2021, said CC BY 4.0) was seen only in a search snippet; hri.fi and avoindata.fi answered 403 and were not bypassed.

Sofia (CITY-BETTER, modest)
- Open-data CKAN (publisher Софияплан): "Цифров модел на релефа - чист (София град)" (raster, current to 2017-01-01), "... с включени сгради" (2017) and an elevation map dated 2021-11-18; licence field "Не са зададени лицензни права"; direct download. The archive https://api.sofiaplan.bg/datasets/543 (22.6 MB 7z, read whole) holds one float32 GeoTIFF, 5 m pixels, EPSG:32634 (UTM 34N), 3,801 x 3,452 px covering 680,928-699,933 E by 4,719,444-4,736,704 N (about 19.0 x 17.3 km, 328 km2: a part of the municipality, not all of it), heights 506-1,023 m, file dated 19 Mar 2020, no vertical datum, no licence in the file; the file name suggests contour interpolation (inference). https://urbandata.sofia.bg/api/3/action/package_show?id=digital-terrain-model-clean-surface-sofia-city ; https://urbandata.sofia.bg/api/3/action/package_show?id=digital-surface-model-including-buildings-sofia-city ; https://urbandata.sofia.bg/api/3/action/package_show?id=elevation-map-of-sofia-municipality-in-meters ; https://urbandata.sofia.bg/api/3/action/package_search?q=lidar ; https://api.sofiaplan.bg/datasets/543 . Better in access than Bulgaria's view-only WMS, coarse and unlicensed.

Warsaw and Kraków (national, free)
- GUGiK accepted city-commissioned data: Warsaw point cloud >= 12 pts/m2, NMT 1 m, NMPT 0.5 m; Kraków (post dated 2023-12-11, for the city office) point cloud >= 10 pts/m2, NMT 1 m, NMPT 0.5 m; "Wszystkie powyższe dane można pobrać bezpłatnie". https://www.geoportal.gov.pl/aktualnosci/nowe-dane-fotogrametryczne-w-pzgik-13/ ; https://www.geoportal.gov.pl/aktualnosci/nowe-dane-fotogrametryczne-w-pzgik-10/ ; https://www.geoportal.gov.pl/aktualnosci/nowe-dane-fotogrametryczne-dostepne-w-centralnym-zasobie-geodezyjnym-i-kartograficznym/ . The Warsaw post has no date I could confirm. Warsaw's own sites timed out.

Nothing better than the national source
- Oslo: Geonorge catalogue API searches (laser, terrain model, height model, LiDAR with Oslo; exact query URLs not recorded) show only national Kartverket products (Oslo kommune publishes "HC-plasser"); Oslo's ArcGIS Hub search returned 0. https://opendata.arcgis.com/api/v3/search?q=oslo%20terreng&filter%5Bsource%5D=oslo . A 2014 Oslo thinned height record now returns 404.
- Copenhagen: the city's 3D model of 2013 (terrain as DWG; no licence set). https://www.kk.dk/om-kommunen/fakta-og-statistik/kort-og-data ; https://admin.opendata.dk/api/3/action/package_show?id=kobenhavn-3d-bymodel
- Reykjavik: contour-line layer "Hæðarlínur í Reykjavík" with lidar-derived lines (2023) among older photogrammetry; no raster; licence "Other (Open)". https://lukrgatt.reykjavik.is/server/rest/services/OpinGognGrunngogn/Haedarlinur/MapServer?f=json ; https://gagnagatt.reykjavik.is/api/3/action/package_show?id=69c98b4c-de8f-4227-9464-40ccc6ff2c38
- Tallinn: geoportal offers basemap, buildings, addresses, no elevation (Wayback); ArcGIS REST root has no relief service. https://web.archive.org/web/20221002062749/https://www.tallinn.ee/en/tallinn-geoportal ; https://gis.tallinn.ee/arcgis/rest/services?f=json
- Riga: open data (78 Rīgas dome datasets) has LOD1/LOD2 buildings, no DEM or lidar. https://data.gov.lv/dati/lv/api/3/action/package_search?fq=organization:riga&rows=200 . The 2023-24 3D reality project (55 km2) lists a classified point cloud and DTM among products, only LOD2 planned open. https://lvportals.lv/dienaskartiba/360234-izstradats-augstas-izskirtspejas-rigas-3d-realitates-un-3d-vektoru-modelis-2024 ; LGIA photorealistic 3D of Riga (0.07 m mesh, 2021): https://lgia.gov.lv/en/rigas-pilsetas-teritorijas-fotorealistiskie-3-dimensiju-modela-dati
- Ljubljana: Mestna občina Ljubljana datasets on podatki.gov.si hold no elevation. https://podatki.gov.si/api/3/action/package_search?q=Ljubljana
- Zagreb: ZG3D 2022 buildings use 2022 lidar but no DEM or cloud is offered; "Otvorena dozvola". https://data.zagreb.hr/api/3/action/package_show?id=zg3d-2022-3d-model-gz ; https://data.zagreb.hr/api/3/action/package_search?q=model ; https://geoportal.zagreb.hr/ ; national DGU relief model on data.gov.hr: https://data.gov.hr/ckan/api/3/action/package_show?id=digitalni-model-reljefa
- Athens, Thessaloniki: city CKAN/DKAN searches for lidar, terrain, relief returned 0; geodata.gov.gr elevation topic holds only contours (Wayback). https://opendata.cityofathens.gr/api/3/action/package_search ; https://opendata.thessaloniki.gr/api/1/search ; https://web.archive.org/web/20251118194718/https://geodata.gov.gr/en/dataset?groups=elevation
- Kyiv: 97 datasets, none for relief or lidar. https://data.kyivcity.gov.ua/api/3/action/package_search
- Yerevan: national CKAN has only global layers (SRTM etc.). https://data.opendata.am/api/3/action/package_search
- Nicosia / Cyprus: DLS data page lists contours free, no DTM; the DLS e-service remains the route. https://portal.dls.moi.gov.cy/en/application_forms/diathesi-geografikon-psifiakon-dedomenon/ . A Eurogeographics presentation (https://eurogeographics.org/app/uploads/2026/02/KLITO_PCC_GIS_Presentation_05052026.pdf) is said in a snippet to describe nationwide lidar at 4 pts/m2; it returned a 404 body and is unverified.
- Belgrade: national portal data.gov.rs has no lidar or terrain-model dataset (0 hits); Belgrade's own portal did not answer. https://data.gov.rs/api/1/datasets/

Unverified units
- Vilnius (opendata.vilnius.lt Cloudflare challenge, not bypassed; other hosts timeout; Wayback shell: https://web.archive.org/web/2025/https://opendata.vilnius.lt/), Budapest (all portals timed out; https://geoportal.budapest.hu), Bratislava (data.bratislava.sk is a single-page app; CKAN path 404), Istanbul (listing only: https://web.archive.org/web/20251208193317/https://data.ibb.gov.tr/dataset/), Moscow (https://web.archive.org/web/20260804220818/https://data.mos.ru/opendata), St Petersburg (https://web.archive.org/web/20211120034326/https://data.gov.spb.ru/opendata/), Minsk (https://web.archive.org/web/20211226143108/https://data.gov.by/ , a stub), Ankara, Tbilisi, Baku, Chişinău, Tatarstan, Sakhalin, Kamchatka, Yakutia: no product seen; the hosts timed out or did not resolve and Wayback had listing pages only (no capture for Tatarstan, Kamchatka, Yakutia roots). Search snippets mention a Tatarstan "ЦМРТ" project and a closed Sakhalin urban-planning digital twin; neither is an open elevation product.
- Federal subjects: no subject was found publishing its own elevation data; they share one UNVERIFIED row in 2.5.

## 4. Non-governmental sources (what differs for this region)

| Source | Type | What for this region | Beats national or global? | Licence | Offline cache | URL |
|---|---|---|---|---|---|---|
| NON-GOVERNMENTAL: Mapterhorn | open-source project (NLnet NGI0 funded; maintainer Oliver Wipfli) | Terrain tiles from open national data; its coverage list names for this region: Cyprus 1 m, Czechia 5 m, Denmark 0.4 m (Faroe 10 m), Estonia 1 m, Finland 2 m, Iceland 10 m, Latvia 20 m, Norway 1 m (Svalbard 20 m), Poland 5 m country-wide plus 1 m partial, Romania partial 0.5 m, Slovakia 1 m, Slovenia 0.5 m, Sweden 1 m; GLO-30 elsewhere. Its attribution JSON lists the producer, licence, access year and raw size per source (for example Poland "Open Government Data (Art. 40a)", Sweden CC0, Estonia "opendata licence 2025", Romania OGL-ROU-1.0) | Same data as the national sources, in one tile format; useful because most are packaged for offline use. It does not carry Hungary, Croatia, Serbia, Bulgaria, Greece, Turkey, Ukraine, Belarus, Russia, Lithuania | per source (CC BY 4.0, CC0, OGL-ROU-1.0, national open-data licences) | yes under each source licence with credit | https://mapterhorn.com/ ; https://mapterhorn.com/attribution/ ; https://download.mapterhorn.com/attribution.json |
| NON-GOVERNMENTAL: Sonny's LiDAR DTMs of Europe | individual compiler | resampled DTMs for Norway (with Svalbard, Jan Mayen), Sweden, Finland, Denmark, Faroes, Estonia, Lithuania ("Lithuania v1" 2023), Poland v1, Czechia v2, Hungary v1, Croatia v1, Slovakia v2, Romania, Cyprus; none for the Balkans; 1", 3", 20 m, 50 m only | no (coarser than national); useful for Hungary and Croatia where the official source is paid, but the source data behind them was not readable | CC BY 4.0, credit and link to Sonny | yes (licence permits copying) | https://sonny.4lima.de/ |
| NON-GOVERNMENTAL: ArcticDEM (Polar Geospatial Center, University of Minnesota) | academic | 2 m DSM mosaics and strips for the Arctic: Svalbard, Iceland, Faroes, northern Fennoscandia, Arctic Russia | yes in the far north over GLO-30 (2 m vs 30 m), though a surface model without ground control | CC-BY-4.0 in the STAC collection; the page also mentions an "EOCL" commercial-data licence and public access continuing "except Alaska" (FAQ not read) | likely yes (CC-BY-4.0) | https://www.pgc.umn.edu/data/arcticdem/ ; https://pgc-opendata-dems.s3.us-west-2.amazonaws.com/arcticdem/mosaics.json |
| NON-GOVERNMENTAL: rapidlasso (LAStools vendor) | company blog | reports Latvia's whole-country LAS tiles as CC0 1.0 (2019) | confirms the open Latvian points | CC0 1.0 as reported | yes if the points are still CC0; the official LĢIA licence was not confirmed | https://rapidlasso.de/national-open-lidar-strategy-of-latvia-humiliates-germany-austria-and-other-european-closed-data-states/ |
| NON-GOVERNMENTAL: windPRO / EMD wiki | commercial software vendor | descriptions and resamples of national DEMs for Denmark, Faroes, Iceland, Latvia, Norway, Romania | no | windPRO terms not read | not a bulk source | https://help.emd.dk/mediawiki/index.php/Danish_Elevation_Model |
| NON-GOVERNMENTAL: technicalgis.com LiDAR atlas | third-party catalogue (pointer only) | lists Poland, Slovakia, Slovenia open; Hungary, Croatia, Kosovo, Albania restricted; Bosnia, Serbia, Montenegro, North Macedonia "none known"; its 0.5 m / 5 m figures are not backed by publisher pages | no | n/a | n/a | https://www.technicalgis.com/lidar-elevation-atlas/ |
| NON-GOVERNMENTAL: source.coop Moldova catalogue (community) | community | links to the live Moldovan WMS layers; mirrors no data | no | n/a | no | https://source.coop/nlebovits/moldova-geodata/wms/dtm/AGENTS.md |
| NON-GOVERNMENTAL: ALOS AW3D30 (JAXA) and Viewfinder Panoramas | agency product / individual | global 30 m DSM (AW3D30, free for commercial and non-commercial use under JAXA terms); Viewfinder 1"/3" built from public sources (licence not read) | no | JAXA terms (section 5 of them not read) | AW3D30 likely yes; Viewfinder unconfirmed | https://www.eorc.jaxa.jp/ALOS/en/dataset/aw3d30/aw3d30_e.htm ; https://www.viewfinderpanoramas.org/dem3.html |
| OpenTopography | academic facility | programmatic access to open DEMs including Copernicus; no regional dataset for this region was read | no | per dataset | per dataset | https://opentopography.org/ |
| Commercial DEMs (Airbus WorldDEM Neo, Intermap, Vantor, Maxar) | commercial | see terrain-europe-w.md section 4.2; nothing region-specific was read in this run | paid, not offline-cacheable without a licence | | no | not re-read |

Bottom line: for a personal offline map the best route in the Nordics, Baltics (Estonia), Slovakia, Slovenia, Poland and Cyprus is the national open DTM, either from the publisher or from Mapterhorn's PMTiles of it. Nothing non-governmental gives finer data than the national sources.

## 5. Unverified (each with the routes tried)

- Lithuania: licence/terms and free-download status of the national lidar (the catalogue records say only "copyright"/"license"); the actual LAZ sheet URLs and tile index (not seen; coverage PNGs only). Routes tried: three geoportal.lt pages, the CSW records, data.gov.lt API (HTTP 500 on three paths), Wayback availability (no snapshot), Wayback NŽT page; NŽT sites 403 (not bypassed).
- Poland: live GUGiK hosts (mapy/opendata/integracja/api.dane.gov.pl) timed out; read through geoportal.gov.pl pages and a Wayback WFS capabilities copy; vertical accuracy and the live indexes not read; voivodeship and city portals not checked.
- Slovenia: official ARSO LiDAR page ("temporarily unavailable"), eprostor live host (timeout), CLSS access terms and tile index; routes: Wayback, vendor pages, podatki.gov.si.
- Croatia: the DGU / NIPP conflict is quoted in the Croatia note; unresolved: whether the lidar point cloud can actually be downloaded without a request, and the edit dates of both pages (neither prints one; NN 106/25's date not fetched). Zagreb's possible 2012 DMR WMS (snippet only; INSPIRE record unreachable).
- Hungary: county and city portals, FÖMI/Geoshop price lists not checked.
- Montenegro (geoportal.co.me, katastar.co.me timeouts; Wayback empty), Kosovo (JS geoportal), North Macedonia (live hosts timed out; Wayback read; terms not read), Albania (download terms, DTM grid), Brčko District (bdcentral.net timed out).
- Czechia: Brno DMT 2019 page did not render; Prague cell size not read.
- Romania: national MNT grid, geoportal.ancpi.ro, geoportal.gov.ro, data.gov.ro (DNS/timeouts; Wayback empty shell); county and city lidar.
- Bulgaria: grid and licence of the view-only layers; cadastre, data.gov.bg and the live INSPIRE geoportal did not resolve.
- Greece: lidar programme details (area, density, contract) only in Eurogeographics slides that now return 404 and a Greek tender PDF that could not be decoded; ktimatologio.gr and geodata.gov.gr unreachable (403 / no DNS).
- Cyprus: lidar density and accuracy (Eurogeographics presentation 404, no Wayback copy); the e-service's tile list needs a login (not attempted); DTM resolution rests on Mapterhorn's list.
- Turkey: HGM product detail pages for resolution and area, TKGM lidar guide (PDF not decodable), Istanbul and other metropolitan portals (no connection).
- Georgia, Armenia, Azerbaijan: no official elevation data found; ARKA article on Armenia's 2021 survey returned 403 live and via Wayback; a Georgian lidar plan appears only in an ADB slide deck that returned 404.
- Ukraine, Belarus, Russia: map.land.gov.ua, geoportal.gov.ua, nca.by, map.gov.by, rosreestr.gov.ru, nspd.gov.ru and most Russian regional portals timed out; dzz.by returned a Cloudflare block (not bypassed); Kyiv city portal 403. Wayback was tried for the Russian portals (home pages only) and nca.by (no terrain text). Sakhalin, Kamchatka and Yakutia portals not read. The FFPD availability page and price list text did not load. These "nothing found" results are absences of evidence.
- Sweden: vertical accuracy and density of the Markhöjdmodell (product pages 404; Geodataportalen record and STAC give none).
- Finland: whether the MapSite browser download (as opposed to the WCS / file service, which need the free key per the NLS key page) needs a sign-in; not tested.
- Denmark: live 2025 figures (only a third party), Dataforsyningen data pages (JS app), a no-login bulk route.
- Faroe Islands: the English licence page (404); licence wording from a third party and Mapterhorn.
- Iceland: positional accuracy (search snippet only, not used), a bulk URL for the 2 m tiles.
- Estonia: lidar density (no page states it).
- Latvia: whether CC0 is the official licence; where the open LAS tiles are listed.
- Svalbard: coverage share of DTM 5 (the record says "parts of Svalbard"); ArcticDEM licence FAQ (EOCL).
- City and regional pass: Helsinki point cloud and licence (hri.fi and avoindata.fi 403, Wayback no snapshot) and 50 cm vs 1 m; Riga 3D/DTM download (georiga.eu timed out); Stockholm licence; Gothenburg zip URLs and DTM/DSM nature; Brno lidar density; Vilnius, Budapest, Bratislava, Belgrade, Bucharest (pmb.ro hosts), Chişinău, Minsk, Ankara, Istanbul (listing only), Tbilisi, Baku, Brčko, Tatarstan/Sakhalin/Kamchatka/Yakutia (hosts unreachable; Wayback listing pages only, routes in note 3.5); Brčko's only lead is the national IPA lidar; Cyprus lidar claim from an unreadable Eurogeographics PDF; the RGURS licence (empty abstracts).
- Pan-European products: not re-checked (see terrain-europe-w.md section 4).

## 6. Anything accidental

- Åland agent: downloaded a 6.5 MB PDF (government protocol); not used as a source. It also read a 2.6 MB LĢIA PDF and ran one Python (without -I) from / to read it with a user-site pypdf; no downloaded code was executed.
- Ukraine/Belarus/Russia agent: one 1.0 MB PDF full read, a 678 KB WMS capabilities document, and two community pages (213 and 275 KB); two shell loops ran past 120 s into the background and finished. It did not delete its scratch folder; I deleted it.
- Balkans agent: a 64x64 px GetMap test against the Republika Srpska WMS (436-byte exception); downloaded into scratch a 1.2 MB Croatian lidar spec PDF, a 0.9 MB journal PDF, a 0.37 MB atlas page and a 1.27 MB docs page; all deleted. Some hosts needed a cookie jar or dummy session path to avoid redirect loops (no login bypassed).
- SE Europe agent: no oversized reads; WebFetch saved four PDFs (0.5-2.3 MB) in its own tool-results folder, left untouched.
- Me: read attribution.json from download.mapterhorn.com (95 KB, under the cap) and the Mapterhorn home and attribution pages.
- Blocks respected: dzz.by (Cloudflare), nzt.lt (403), zerkalo.io (403), ktimatologio.gr (403), data.gov.lt firewall page, ARKA (403). Hosts that timed out were not retried beyond a handful of attempts.
- City pass: the Nordic/Central city agent used WebFetch as a second route after hri.fi returned 403 (a different client; minor deviation, no further attempts), printed a config.js to the shell, and compiled a small PDFKit reader in scratch (deleted). Four tiny anonymous GetMap probes were sent to the RGURS WMS (errors only). I paged the geoportal.lt CSW (four 190 KB answers) and read 800 titles.
- Search was used only to discover URLs.

## 7. URLs cited

Every URL below appears in this file and was reported as fetched with a 2xx answer in this run (HEAD for the tile and ZIP files marked in the notes). A few URLs the notes mark as "named on the page, not fetched" (Estonian footprint links, Slovak cycle-1 folder, Polish live index endpoints, ČÚZK sheet-grid candidates beyond those read) are named there and not claimed as fetched.

Also read whole: https://api.sofiaplan.bg/datasets/543 ; https://dataportalen.stockholm.se/dataportalen/Data/Stadsbyggnadskontoret/Testdata_Punktmoln_2025.las ; https://geoportaal.maaruum.ee/docs/Avaandmed/ETAK_open_data_licence.pdf (the opendata-licence redirect target) ; https://geoportaal.maaamet.ee/docs/pohikaart/lidar_aasta.jpg. City-pass additions are cited in 2.5 and 3.5; URLs that were only named (not fetched) are excluded from this list.

- https://www.kartverket.no/en/api-and-data/terrengdata
- https://kartkatalog.geonorge.no/api/getdata/010bfb4a-65aa-4d30-aba9-7d090a432df7
- https://kartkatalog.geonorge.no/api/getdata/731a0602-d262-483d-aaa6-474cef68e82a
- https://api.lantmateriet.se/stac-hojd/v1/collections/dtm-cog
- https://www.maanmittauslaitos.fi/en/maps-and-spatial-data/datasets-and-interfaces/product-descriptions/elevation-model-2-m
- https://kartor.regeringen.ax/arcgis/rest/services/Topografi/Hojd/MapServer?f=json
- https://eng.kds.dk/green-transition-and-climate-adaptation/the-danish-elevation-model-dk-dem
- https://www.foroyakort.fo/tak-datur-nidur/dtm-og-dsm-haeddarmodell/
- https://gatt.natt.is/geonetwork/srv/api/records/e6712430-a63c-4ae5-9158-c89d16da6361/formatters/xml
- https://geoportaal.maaamet.ee/eng/spatial-data/elevation-data-p308.html
- https://www.lgia.gov.lv/en/digital-height-models-0
- https://www.geoportal.lt/metadata-catalog/csw?service=CSW&version=2.0.2&request=GetRecordById&elementSetName=full&outputSchema=http://www.isotc211.org/2005/gmd&id=%7B3C0D5E47-49CA-4132-88C1-EC3D1E4610B4%7D
- https://www.geoportal.gov.pl/pl/dane/numeryczny-model-terenu-nmt/
- https://geoportal.cuzk.gov.cz/(S(1))/Default.aspx?mode=TextMeta&side=dmr5g&metadataID=CZ-CUZK-DMR5G-V&menu=302
- https://www.arcgis.com/sharing/rest/content/items/97d2c9c11aa9478cb21b469b8a4f820e?f=json
- https://www.skgeodesy.sk/geoportal/zbgis/lls/poskytovanie/
- https://lechnerkozpont.hu/node/1963
- https://web.archive.org/web/2026/https://eprostor.gov.si/imps/srv/api/records/951eaa88-0f6b-4d29-8d02-eaf56534453e/formatters/xml
- https://dgu.gov.hr/proizvodi-i-usluge/podaci-topografske-izmjere/digitalni-model-reljefa/180
- https://metapodaci.ippfbih.gov.ba/srv/api/records/2cfd6c83-748e-45ec-acc8-a9729e3173dc
- https://geoportal.rgurs.org/init/geoportal.json
- https://www.klix.ba/scitech/nauka/bosna-i-hercegovina-uskoro-dobija-prvu-jedinstvenu-3d-kartu-zemlje-zavrseno-lidar-snimanje/260825121
- https://www.rgz.gov.rs/digitalni-model-terena
- https://www.gim-international.com/content/news/dtp-and-dop-for-entire-montenegro
- https://www.kca-ks.org/
- https://web.archive.org/web/2026/https://aknlidarappprod.katastar.gov.mk/LiDARInfoStrana/index.html
- https://asig.gov.al/en/aerial-photography-in-the-republic-of-albania/
- https://help.emd.dk/mediawiki/images/c/ca/Licen%C8%9Ba_deschisa_Modelul_numeric_al_terenului_v2_MNT.pdf
- http://inspire.mzh.government.bg:8080/geoserver/ows?service=wms&version=1.3.0&request=GetCapabilities
- https://web.archive.org/web/2025/https://www.gov.gr/ipiresies/periousia-kai-phorologia/ktematographese/geopule-geokhorikon-dedomenon-inspire-ellenikou-ktematologiou
- https://portal.dls.moi.gov.cy/en/?p=18685
- https://sabah.com.tr/ekonomi/kktcnin-dijital-ikizi-cikarilacak-7367458
- https://web.archive.org/web/2025/https://www.harita.gov.tr/urunler/yukseklik-verileri-grid/1
- https://napr.gov.ge/
- https://cadastre.am/en/cadastre_mapping
- https://dataspace.copernicus.eu/explore-data/data-collections/copernicus-contributing-missions/collections-description/COP-DEM
- https://registry.opendata.aws/copernicus-dem/
- https://ggim.un.org/meetings/2019/IWS/Minsk/documents/S2-7%20Belarus-Marina%20Litreeva.pdf
- https://geodata.gov.md/geoserver/ows?service=WMS&request=GetCapabilities
- https://web.archive.org/web/20260510121956/https://inspire-geoportal.ec.europa.eu/srv/api/records/38dac355-7cc1-4110-ab04-7406e3e5fbb5
- https://web.archive.org/web/20241206110844/https://cgkipd.ru/fsdf/service/provide/
- https://pgc-opendata-dems.s3.us-west-2.amazonaws.com/arcticdem/mosaics.json
- https://web.archive.org/web/20260106145652/https://fpd.tatarstan.ru/
- https://gis.brno.cz/ags1/rest/services/OMI/OMI_DMT_2019_elevation/ImageServer?f=json
- https://opengeodata.goteborg.se/files/hojdmodell/2022/hojdmodell_2022_sv.pdf
- https://dataportalen.stockholm.se/dataportalen/srv/eng/csw
- https://kartta.hel.fi/ws/geoserver/avoindata/wcs?service=WCS&request=GetCapabilities
- https://urbandata.sofia.bg/api/3/action/package_show?id=digital-terrain-model-clean-surface-sofia-city
- https://www.geoportal.gov.pl/aktualnosci/nowe-dane-fotogrametryczne-w-pzgik-10/
- https://lvportals.lv/dienaskartiba/360234-izstradats-augstas-izskirtspejas-rigas-3d-realitates-un-3d-vektoru-modelis-2024
- https://geoportal.rgurs.org/geoserver/dmt/wms?service=WMS&request=GetCapabilities
- https://metapodaci.ippfbih.gov.ba/srv/eng/xml.metadata.get?uuid=2cfd6c83-748e-45ec-acc8-a9729e3173dc
- https://www.kartverket.no/en/api-and-data/terms-of-use
- https://kartkatalog.geonorge.no/api/getdata/f297e948-8a34-4e6c-9740-54b3a657f8d5
- https://kartkatalog.geonorge.no/api/search?text=nasjonal%20detaljert%20h%C3%B8ydemodell&limit=8
- https://kartkatalog.geonorge.no/api/getdata/35e66707-942c-4aa2-9945-d769c2aeba35
- https://wcs.geonorge.no/skwms1/wcs.hoyde-dtm-nhm-25833?service=WCS&request=GetCapabilities
- https://kartkatalog.geonorge.no/api/getdata/0f0a0f38-00c4-4213-a9e5-2d861dc4abb0
- https://wfs.geonorge.no/skwms1/wfs.hoyde-hoydedata-metadata-prosjekt?service=wfs&request=getcapabilities
- https://kartkatalog.geonorge.no/api/getdata/18c447cb-24d0-4e38-b887-e279d66fbd0c
- https://nedlasting.geonorge.no/geonorge/Tjenestefeed.xml
- https://kartkatalog.geonorge.no/api/search?text=digital%20terrengmodell%201%20meter&limit=8
- https://kartkatalog.geonorge.no/api/getdata/9d118d31-182c-495b-b7be-d819cc7444b1
- https://kartkatalog.geonorge.no/api/search?limit=15&text=Svalbard%20laser%20punktsky
- https://www.pgc.umn.edu/data/arcticdem/
- https://data.pgc.umn.edu/elev/dem/setsm/ArcticDEM/indexes/ArcticDEM_Mosaic_Index_latest_gpkg.zip
- https://stac.pgc.umn.edu/api/v1/
- https://overlord.pgc.umn.edu/arcgis/rest/services/elevation/pgc_arcticdem_mosaics_latest/ImageServer?f=json
- https://api.lantmateriet.se/stac-hojd/v1/
- https://api.lantmateriet.se/stac-hojd/v1/collections/dtm-cog/items
- https://api.lantmateriet.se/stac-hojd/v1/collections
- https://www.lantmateriet.se/globalassets/geodata/geodataprodukter/anvandningsvillkor-for-laserdata-nedladdning-skog.pdf
- https://www.geodata.se/geodataportalen/srv/api/records/1b48584d-f6d6-43fb-a6fd-7610a33de491/formatters/xml
- https://www.lantmateriet.se/sv/geodata/vara-produkter/avgiftsfria-produkter/oppna-data/
- https://tilannekartta.maanmittauslaitos.fi/km2
- https://www.maanmittauslaitos.fi/en/maps-and-spatial-data/datasets-and-interfaces/product-descriptions/laser-scanning-data-05-p
- https://www.maanmittauslaitos.fi/en/maps-and-spatial-data/datasets-and-interfaces/product-descriptions/laser-scanning-data-5-p
- https://www.maanmittauslaitos.fi/laserkeilausaineistot
- https://www.maanmittauslaitos.fi/laserkeilausaineistot/kayttoehdot
- https://www.maanmittauslaitos.fi/laserkeilaus-ja-ilmakuvaus
- https://www.maanmittauslaitos.fi/sites/maanmittauslaitos.fi/files/attachments/2026/01/KALLIO2_keilausohjelma_5p.zip
- https://www.maanmittauslaitos.fi/sites/maanmittauslaitos.fi/files/attachments/2026/01/KALLIO2_keilausohjelma_20p.zip
- https://www.maanmittauslaitos.fi/en/rajapinnat/api-avaimen-ohje
- https://www.maanmittauslaitos.fi/en/maps-and-spatial-data/datasets-and-interfaces/general-information-about-interface-services
- https://www.maanmittauslaitos.fi/rajapinnat/paikkatiedot
- https://kartor.regeringen.ax/arcgis/rest/services?f=json
- https://www.kartor.ax/api/search/v1/collections/all/items?limit=100
- https://eng.kds.dk/mapping/aerial-photography-and-lidar-scanning
- https://cis2.eea.europa.eu/data/481
- https://help.emd.dk/mediawiki/index.php/Danish_Elevation_Model
- https://kds.dk/om-klimadatastyrelsen/vilkaar-og-priser
- https://docs.dataforsyningen.dk/
- https://help.emd.dk/mediawiki/index.php/Faroe_Islands_Elevation_Models
- https://gis.us.fo/arcgis/rest/services?f=json
- https://gis.us.fo/arcgis/rest/services/dsm?f=json
- https://gis.us.fo/arcgis/rest/services/haeddarkurvar?f=json
- https://gis.natt.is/mapcache/web-mercator/wms?request=GetCapabilities
- https://ftp.natt.is/gisdata/raster/
- https://help.emd.dk/mediawiki/index.php/Iceland_Elevation_Model
- https://geoportaal.maaamet.ee/eng/spatial-data/elevation-data/download-elevation-data-p664.html
- https://geoportaal.maaamet.ee/opendata-licence
- https://www.lgia.gov.lv/en/Digit%C4%81lais%20virsmas%20modelis
- https://lgia.gov.lv/sites/lgia/files/document/Kartogr%C4%81fu_diena_2025_pakalpojumi.pdf
- https://rapidlasso.de/national-open-lidar-strategy-of-latvia-humiliates-germany-austria-and-other-european-closed-data-states/
- https://www.geoportal.lt/metadata-catalog/csw?service=CSW&version=2.0.2&request=GetRecordById&elementSetName=full&outputSchema=http://www.isotc211.org/2005/gmd&id=%7BFEB7F0A6-3BD4-4BF9-AE93-4A4380904E6D%7D
- https://www.geoportal.lt/metadata-catalog/csw?service=CSW&version=2.0.2&request=GetCapabilities
- https://www.geoportal.lt/download/Matadata/Images/NZT/lidar/Lidar_DR_LT.png
- https://www.geoportal.lt/download/Matadata/Images/NZT/lidar/Lidar_DR_LT_recent.png
- https://www.geoportal.lt/metadata-catalog/csw?service=CSW&version=2.0.2&request=GetRecordById&elementSetName=full&outputSchema=http://www.isotc211.org/2005/gmd&id=%7B64A10467-1F79-64C2-7C01-8511AA1F234D%7D
- https://www.geoportal.lt/geoportal/web/inspire-en/data-availability
- https://www.geoportal.lt/geoportal/web/en/open-data
- https://www.geoportal.lt/geoportal/web/en/download-services
- https://archive.org/wayback/available?url=data.gov.lt/api/3/action/package_search?q=lidar
- https://web.archive.org/web/20241125215406/https://nzt.lrv.lt/lt/atviri-duomenys/
- https://www.geoportal.gov.pl/pl/dane/dane-pomiarowe-lidar-lidar/
- https://www.geoportal.gov.pl/pl/dane/numeryczny-model-pokrycia-terenu-nmpt/
- https://web.archive.org/web/2024/https://mapy.geoportal.gov.pl/wss/service/PZGIK/NumerycznyModelTerenuEVRF2007/WFS/Skorowidze?service=WFS&request=GetCapabilities
- https://geoportal.cuzk.gov.cz/(S(1))/Default.aspx?mode=TextMeta&side=dmr5g&metadataID=CZ-CUZK-DMR4G-V&menu=302
- https://geoportal.cuzk.gov.cz/(S(1))/Default.aspx?mode=TextMeta&side=dmr5g&metadataID=CZ-CUZK-DMP1G-V&menu=302
- https://geoportal.cuzk.gov.cz/(S(1))/Default.aspx?mode=TextMeta&side=vyskopis&menu=3
- https://atom.cuzk.gov.cz/
- https://ags.cuzk.gov.cz/arcgis/rest/services/KladyMapovychListu/MapServer?f=pjson
- https://ags.cuzk.gov.cz/arcgis/rest/services?f=pjson
- https://geoportalpraha.cz/data-a-sluzby/otevrena-data
- https://gis.brno.cz/ost/edas/public/7517ffb8-6adc-4346-b87b-f37ffdc27f16
- https://www.skgeodesy.sk/geoportal/zbgis/lls/
- https://www.skgeodesy.sk/geoportal/zbgis/lls/1-cyklus/
- https://opendata.skgeodesy.sk/static/LLS/Footprints/2_cyklus/las_footprints_etrs89-tm34_2_cyklus_gdb.zip
- https://clss.si/
- https://www.e-prostor.gov.si/podrocja/drzavni-topografski-sistem/digitalni-modeli-visin/
- https://podatki.gov.si/api/3/action/package_show?id=digitalni-model-visin
- https://www.kingprostor.si/blog/arso-lidar/
- https://site.geo-portal.si/2021-01-04-vkljucitev-lidar-2.html
- https://gis.arso.gov.si/evode/profile.aspx?id=atlas_voda_Lidar%40Arso
- https://registri.nipp.hr/izvori/1871
- https://dgu.gov.hr/multisenzorsko-zracno-snimanje-republike-hrvatske/5700
- https://www.lifecrolis.hr/en/2024/10/18/the-normalized-digital-surface-model-of-croatia-is-available-for-use/
- https://dgu.gov.hr/UserDocsImages/dokumenti/Istaknute%20teme/Multisenzorno%20snimanje/LiDAR%20snimanje%20iz%20zraka.pdf
- https://katastar.ba/koristenje
- https://metapodaci.ippfbih.gov.ba/srv/eng/q?_content_type=json&fast=index&any=lidar
- https://metapodaci.ippfbih.gov.ba/srv/eng/xml.metadata.get?uuid=e555a373-eb44-4582-a93d-2557ca4255d9
- https://metapodaci.ippfbih.gov.ba/srv/eng/xml.metadata.get?uuid=097737b8-a0cd-4e7a-ba25-2f019aeab8fc
- https://metapodaci.ippfbih.gov.ba/srv/eng/xml.metadata.get?uuid=c01a42cf-a851-4d2d-8424-25646045621f
- https://metapodaci.ippfbih.gov.ba/srv/eng/xml.metadata.get?uuid=adbbd0cd-a68a-43cc-b999-d38d7ace03d4
- https://metapodaci.ippfbih.gov.ba/srv/eng/xml.metadata.get?uuid=f28e113e-d54b-4b78-9f2b-c49b5c2115e9
- https://geoportal.rgurs.org/geoserver/dem/wms?service=WMS&request=GetCapabilities
- https://web.archive.org/web/2025/https://fgu.com.ba/en/the-surveying-project-of-bosnia-and-herzegovina-territory-lidar-ipa-ii-2019.html
- https://web.archive.org/web/20260107004209/https://vlada.bdcentral.net/
- https://web.archive.org/web/2026/https://vlada.bdcentral.net/Content/Read/sef-odjeljenja-za-javni-registar
- https://metapodaci.ippfbih.gov.ba/srv/api/records/adbbd0cd-a68a-43cc-b999-d38d7ace03d4
- https://katastar.ba/
- https://geoportal.rgurs.org/config.json
- https://www.rgurs.org/
- https://geoportal.rks-gov.net/
- https://web.archive.org/web/2026/https://www.katastar.gov.mk/en/home/
- https://geoportal.asig.gov.al/en/node/954
- https://help.emd.dk/mediawiki/index.php/Romania_Elevation_Model
- https://lidarmag.com/2024/09/22/elevations-for-the-nations-part-ii/
- https://lidarmag.com/2020/11/01/lidar-initiatives-at-the-national-center-of-cartography-romania/
- https://download.mapterhorn.com/attribution.json
- https://arxiv.org/html/2606.08876v1
- https://web.archive.org/web/2025/https://inspire-geoportal.ec.europa.eu/srv/api/records/a03ad76a-b38e-4aea-b95e-264ebad4f090/formatters/xml
- https://www.cadastre.bg/
- https://lidarmag.com/2025/08/30/elevations-for-the-nations-part-iii/
- https://www.taxheaven.gr/news/57349/kthmatologio-prosbash-se-oloys-sta-anoixta-dedomena-mesw-toy-dataktimatologiogr
- https://portal.dls.moi.gov.cy/en/application_forms/diathesi-geografikon-psifiakon-dedomenon/
- https://portal.dls.moi.gov.cy/en/to-tmima/kladoi/geodaisias-ydografias-fotogrammetrias/tomeas-fotogrammetrias-kai-tilepiskopisis-2/
- https://mapterhorn.com/
- https://www.tkgm.gov.tr/sites/default/files/2025-06/IlanMetni_Kibris_3B.pdf
- https://dergipark.org.tr/en/pub/tuzal/issue/92575/1665238
- https://tucbs.gov.tr/
- https://nsdi.gov.ge/
- https://geology.am/en/Lab-geoinformatics-scientific-achievements/
- https://dataspace.copernicus.eu/node/1306
- https://data.gov.ua/api/3/action/package_search?q=DEM&rows=5
- https://nsdi.gov.ua/
- https://land.gov.ua/
- https://copernicus-dem-30m.s3.amazonaws.com/Copernicus_DSM_COG_10_N50_00_E030_00_DEM/Copernicus_DSM_COG_10_N50_00_E030_00_DEM.tif
- https://copernicus-dem-30m.s3.amazonaws.com/Copernicus_DSM_COG_10_N46_00_E035_00_DEM/Copernicus_DSM_COG_10_N46_00_E035_00_DEM.tif
- https://elib.bsu.by/bitstream/123456789/308729/1/128-134.pdf
- https://copernicus-dem-30m.s3.amazonaws.com/Copernicus_DSM_COG_10_N53_00_E027_00_DEM/Copernicus_DSM_COG_10_N53_00_E027_00_DEM.tif
- https://geodata.gov.md/geoserver/ows?service=WCS&request=GetCapabilities
- https://geodata.gov.md/geoserver/ows?service=WFS&request=GetCapabilities
- https://source.coop/nlebovits/moldova-geodata/wms/dtm/AGENTS.md
- https://copernicus-dem-30m.s3.amazonaws.com/Copernicus_DSM_COG_10_N47_00_E028_00_DEM/Copernicus_DSM_COG_10_N47_00_E028_00_DEM.tif
- https://capital.ro/oglinda-digitala-a-republicii-moldova-teritoriul-va-fi-scanat-3d-din-avion-pentru-harti-gratuite-de-inalta-precizie.html
- https://web.archive.org/web/20241206110844/https://cgkipd.ru/
- https://web.archive.org/web/20241206110844/https://cgkipd.ru/fsdf/vedenie-ffpd/dostupnost/
- https://web.archive.org/web/20260120160200/https://data.mos.ru/
- https://web.archive.org/web/20250812001052/https://rgis.spb.ru/
- https://web.archive.org/web/20250130125200/https://rgis.mosreg.ru/v3/
- https://web.archive.org/web/20220122143206/https://opendata.krasnodar.ru/
- https://web.archive.org/web/20251229012941/https://data.gov.ru/
- https://copernicus-dem-30m.s3.amazonaws.com/Copernicus_DSM_COG_10_N55_00_E037_00_DEM/Copernicus_DSM_COG_10_N55_00_E037_00_DEM.tif
- https://copernicus-dem-30m.s3.amazonaws.com/Copernicus_DSM_COG_10_N69_00_E033_00_DEM/Copernicus_DSM_COG_10_N69_00_E033_00_DEM.tif
- https://gis.brno.cz/ost/edas/public/scripts/getDetailData.php?uuid=7517ffb8-6adc-4346-b87b-f37ffdc27f16
- https://gis.brno.cz/ost/edas/public/scripts/getCatalogData.php
- https://gis.brno.cz/ost/edas/public/scripts/getDetailData.php?uuid=33339dd5-bdd2-4e47-8fb4-5b5562e3d296
- https://gis.brno.cz/ost/edas/public/scripts/getDetailData.php?uuid=d263c3ad-28d2-4db4-a07f-49651e1fd266
- https://opengeodata.goteborg.se/files/hojdmodell/2022/hojdmodell_2022_en.pdf
- https://goteborg.se/psidata
- https://www.mynewsdesk.com/se/goteborgsstad/pressreleases/stadsbyggnadskontoret-i-goeteborg-slaepper-fler-oeppna-grundlaeggande-geodata-nu-med-projekteringsfokus-3016588
- https://kartor.stockholm/kartor-geodata/
- https://dataportalen.stockholm.se/dataportalen/Data/Stadsbyggnadskontoret/Testdata_Punktmoln_2025.las
- https://kartta.hel.fi/ws/geoserver/avoindata/wcs?service=WCS&version=2.0.1&request=DescribeCoverage&coverageId=avoindata__Korkeusmalli_2021_1m
- https://web.archive.org/web/2024/https://avoindata.fi/data/en/dataset/helsingin-korkeusmalli
- https://kartta.hel.fi/ws/geoserver/avoindata/wms?service=WMS&request=GetCapabilities
- https://www.hel.fi/en/decision-making/information-on-helsinki/maps-and-geospatial-data/helsinki-3d
- https://urbandata.sofia.bg/api/3/action/package_show?id=digital-surface-model-including-buildings-sofia-city
- https://urbandata.sofia.bg/api/3/action/package_show?id=elevation-map-of-sofia-municipality-in-meters
- https://urbandata.sofia.bg/api/3/action/package_search?q=lidar
- https://api.sofiaplan.bg/datasets/543
- https://www.geoportal.gov.pl/aktualnosci/nowe-dane-fotogrametryczne-w-pzgik-13/
- https://www.geoportal.gov.pl/aktualnosci/nowe-dane-fotogrametryczne-dostepne-w-centralnym-zasobie-geodezyjnym-i-kartograficznym/
- https://opendata.arcgis.com/api/v3/search?q=oslo%20terreng&filter%5Bsource%5D=oslo
- https://www.kk.dk/om-kommunen/fakta-og-statistik/kort-og-data
- https://admin.opendata.dk/api/3/action/package_show?id=kobenhavn-3d-bymodel
- https://lukrgatt.reykjavik.is/server/rest/services/OpinGognGrunngogn/Haedarlinur/MapServer?f=json
- https://gagnagatt.reykjavik.is/api/3/action/package_show?id=69c98b4c-de8f-4227-9464-40ccc6ff2c38
- https://web.archive.org/web/20221002062749/https://www.tallinn.ee/en/tallinn-geoportal
- https://gis.tallinn.ee/arcgis/rest/services?f=json
- https://data.gov.lv/dati/lv/api/3/action/package_search?fq=organization:riga&rows=200
- https://lgia.gov.lv/en/rigas-pilsetas-teritorijas-fotorealistiskie-3-dimensiju-modela-dati
- https://podatki.gov.si/api/3/action/package_search?q=Ljubljana
- https://data.zagreb.hr/api/3/action/package_show?id=zg3d-2022-3d-model-gz
- https://data.zagreb.hr/api/3/action/package_search?q=model
- https://geoportal.zagreb.hr/
- https://data.gov.hr/ckan/api/3/action/package_show?id=digitalni-model-reljefa
- https://opendata.cityofathens.gr/api/3/action/package_search
- https://opendata.thessaloniki.gr/api/1/search
- https://web.archive.org/web/20251118194718/https://geodata.gov.gr/en/dataset?groups=elevation
- https://data.kyivcity.gov.ua/api/3/action/package_search
- https://data.opendata.am/api/3/action/package_search
- https://data.gov.rs/api/1/datasets/
- https://web.archive.org/web/2025/https://opendata.vilnius.lt/
- https://geoportal.budapest.hu
- https://web.archive.org/web/20251208193317/https://data.ibb.gov.tr/dataset/
- https://web.archive.org/web/20260804220818/https://data.mos.ru/opendata
- https://web.archive.org/web/20211120034326/https://data.gov.spb.ru/opendata/
- https://web.archive.org/web/20211226143108/https://data.gov.by/
- https://mapterhorn.com/attribution/
- https://sonny.4lima.de/
- https://www.technicalgis.com/lidar-elevation-atlas/
- https://www.eorc.jaxa.jp/ALOS/en/dataset/aw3d30/aw3d30_e.htm
- https://www.viewfinderpanoramas.org/dem3.html
- https://opentopography.org/
