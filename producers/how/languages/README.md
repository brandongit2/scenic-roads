# Languages by territory: how it's made

Made by `producers/languages/cldr-languages.py` in the repository, from Unicode CLDR 47's
territoryInfo (the cldr-json package, `cldr-core/supplemental/territoryInfo.json`), with the
refinements in the script (languages official only regionally, regions whose own language comes first: Swiss cantons, Belgian regions, South Tyrol, Aosta; Hong Kong and Macau read in
Cantonese):

    uv run --project producers python producers/languages/cldr-languages.py <cldr-json>/cldr-core/supplemental/territoryInfo.json --out <folder>

or, to reapply the refinements to the table here:

    uv run --project producers python producers/languages/cldr-languages.py --base inputs/languages/territories.tsv --out <folder>

Add both outputs (`territories.tsv`, `about.toml`) to the languages inputs; the input check takes them in.
