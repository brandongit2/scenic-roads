// This file is generated. Edit build/generate-unicode-data.ts, then run `pnpm run generate-unicode-data`.

/**
 * Returns whether the given codepoint belongs to a script that is written cursively, whose letters
 * therefore cannot be spaced apart.
 */
export function codePointIsInCursiveScript(codePoint: number): boolean {
    return /[\u0600-\u0604\u0606-\u060B\u060D-\u061A\u061C-\u061E\u0620-\u063F\u0641-\u064A\u0656-\u066F\u0671-\u06DC\u06DE-\u070D\u070F-\u074A\u074D-\u077F\u07C0-\u07FA\u07FD-\u07FF\u0840-\u085B\u085E\u0860-\u086A\u0870-\u0891\u0897-\u08E1\u08E3-\u08FF\u1800\u1801\u1804\u1806-\u1819\u1820-\u1878\u1880-\u18AA\uA840-\uA877\uFB50-\uFD3D\uFD40-\uFDCF\uFDF0-\uFDFF\uFE70-\uFE74\uFE76-\uFEFC]|\uD802[\uDEC0-\uDEE6\uDEEB-\uDEF6\uDF80-\uDF91\uDF99-\uDF9C\uDFA9-\uDFAF]|\uD803[\uDD00-\uDD27\uDD30-\uDD39\uDE60-\uDE7E\uDEC2-\uDEC7\uDED0-\uDED8\uDEFA-\uDEFF\uDF30-\uDF59\uDF70-\uDF89\uDFB0-\uDFCB]|\uD805[\uDE60-\uDE6C]|\uD82F[\uDC00-\uDC6A\uDC70-\uDC7C\uDC80-\uDC88\uDC90-\uDC99\uDC9C-\uDC9F]|\uD83A[\uDD00-\uDD4B\uDD50-\uDD59\uDD5E\uDD5F]|\uD83B[\uDE00-\uDE03\uDE05-\uDE1F\uDE21\uDE22\uDE24\uDE27\uDE29-\uDE32\uDE34-\uDE37\uDE39\uDE3B\uDE42\uDE47\uDE49\uDE4B\uDE4D-\uDE4F\uDE51\uDE52\uDE54\uDE57\uDE59\uDE5B\uDE5D\uDE5F\uDE61\uDE62\uDE64\uDE67-\uDE6A\uDE6C-\uDE72\uDE74-\uDE77\uDE79-\uDE7C\uDE7E\uDE80-\uDE89\uDE8B-\uDE9B\uDEA1-\uDEA3\uDEA5-\uDEA9\uDEAB-\uDEBB\uDEF0\uDEF1]/gim.test(String.fromCodePoint(codePoint));
}

/**
 * Returns whether the given codepoint belongs to a script that is written horizontally from right
 * to left.
 */
export function codePointIsInRTLScript(codePoint: number): boolean {
    return /[\u0591-\u05C7\u05D0-\u05EA\u05EF-\u05F4\u0600-\u0604\u0606-\u060B\u060D-\u061A\u061C-\u061E\u0620-\u063F\u0641-\u064A\u0656-\u066F\u0671-\u06DC\u06DE-\u070D\u070F-\u074A\u074D-\u07B1\u07C0-\u07FA\u07FD-\u082D\u0830-\u083E\u0840-\u085B\u085E\u0860-\u086A\u0870-\u0891\u0897-\u08E1\u08E3-\u08FF\uFB1D-\uFB36\uFB38-\uFB3C\uFB3E\uFB40\uFB41\uFB43\uFB44\uFB46-\uFD3D\uFD40-\uFDCF\uFDF0-\uFDFF\uFE70-\uFE74\uFE76-\uFEFC]|\uD802[\uDC00-\uDC05\uDC08\uDC0A-\uDC35\uDC37\uDC38\uDC3C\uDC3F-\uDC55\uDC57-\uDC9E\uDCA7-\uDCAF\uDCE0-\uDCF2\uDCF4\uDCF5\uDCFB-\uDD1B\uDD1F-\uDD39\uDD3F-\uDD59\uDD80-\uDDB7\uDDBC-\uDDCF\uDDD2-\uDE03\uDE05\uDE06\uDE0C-\uDE13\uDE15-\uDE17\uDE19-\uDE35\uDE38-\uDE3A\uDE3F-\uDE48\uDE50-\uDE58\uDE60-\uDE9F\uDEC0-\uDEE6\uDEEB-\uDEF6\uDF00-\uDF35\uDF39-\uDF55\uDF58-\uDF72\uDF78-\uDF91\uDF99-\uDF9C\uDFA9-\uDFAF]|\uD803[\uDC00-\uDC48\uDC80-\uDCB2\uDCC0-\uDCF2\uDCFA-\uDD27\uDD30-\uDD39\uDD40-\uDD65\uDD69-\uDD85\uDD8E\uDD8F\uDE60-\uDE7E\uDE80-\uDEA9\uDEAB-\uDEAD\uDEB0\uDEB1\uDEC2-\uDEC7\uDED0-\uDED8\uDEFA-\uDF27\uDF30-\uDF59\uDF70-\uDF89\uDFB0-\uDFCB\uDFE0-\uDFF6]|\uD83A[\uDC00-\uDCC4\uDCC7-\uDCD6\uDD00-\uDD4B\uDD50-\uDD59\uDD5E\uDD5F]|\uD83B[\uDE00-\uDE03\uDE05-\uDE1F\uDE21\uDE22\uDE24\uDE27\uDE29-\uDE32\uDE34-\uDE37\uDE39\uDE3B\uDE42\uDE47\uDE49\uDE4B\uDE4D-\uDE4F\uDE51\uDE52\uDE54\uDE57\uDE59\uDE5B\uDE5D\uDE5F\uDE61\uDE62\uDE64\uDE67-\uDE6A\uDE6C-\uDE72\uDE74-\uDE77\uDE79-\uDE7C\uDE7E\uDE80-\uDE89\uDE8B-\uDE9B\uDEA1-\uDEA3\uDEA5-\uDEA9\uDEAB-\uDEBB\uDEF0\uDEF1]/gim.test(String.fromCodePoint(codePoint));
}

/**
 * Returns whether the fallback fonts specified by the
 * `localIdeographFontFamily` map option apply to the given codepoint. 
 */
export function codePointUsesLocalIdeographFontFamily(codePoint: number): boolean {
    return /[\u02EA\u02EB\u1100-\u11FF\u2E80-\u2FDF\u3000-\u30FF\u3105-\u312F\u3131-\u318E\u31A0-\u4DBF\u4E00-\uA48C\uA490-\uA4C6\uA960-\uA97C\uAC00-\uD7C6\uD7CB-\uD7FB\uF900-\uFA6D\uFA70-\uFAD9\uFE10-\uFE1F\uFE30-\uFE4F\uFF00-\uFFEF]|\uD81B[\uDFE0-\uDFFF]|[\uD81C-\uD822\uD840-\uD868\uD86A-\uD86D\uD86F-\uD872\uD874-\uD879\uD880-\uD883\uD885-\uD88C][\uDC00-\uDFFF]|\uD823[\uDC00-\uDCD5\uDCFF-\uDD1E\uDD80-\uDDF2]|\uD82B[\uDFF0-\uDFFF]|\uD82C[\uDC00-\uDEFB]|\uD83C[\uDE00-\uDEFF]|\uD869[\uDC00-\uDEDF\uDF00-\uDFFF]|\uD86E[\uDC00-\uDC1D\uDC20-\uDFFF]|\uD873[\uDC00-\uDEAD\uDEB0-\uDFFF]|\uD87A[\uDC00-\uDFE0\uDFF0-\uDFFF]|\uD87B[\uDC00-\uDE5D]|\uD87E[\uDC00-\uDE1D]|\uD884[\uDC00-\uDF4A\uDF50-\uDFFF]|\uD88D[\uDC00-\uDC79]/gim.test(String.fromCodePoint(codePoint));
}

/**
 * Returns whether the given codepoint participates in ideographic line
 * breaking.
 */
export function codePointAllowsIdeographicBreaking(codePoint: number): boolean {
    return /[\u02EA\u02EB\u2E80-\u2FDF\u2FF0-\u303F\u3041-\u3096\u309D-\u309F\u30A1-\u30FA\u30FD-\u30FF\u3105-\u312F\u31A0-\u4DBF\u4E00-\uA48C\uA490-\uA4C6\uF900-\uFA6D\uFA70-\uFAD9\uFE10-\uFE1F\uFE30-\uFE4F\uFF00-\uFFEF]|\uD81B[\uDFE0-\uDFFF]|[\uD81C-\uD822\uD840-\uD868\uD86A-\uD86D\uD86F-\uD872\uD874-\uD879\uD880-\uD883\uD885-\uD88C][\uDC00-\uDFFF]|\uD823[\uDC00-\uDCD5\uDCFF-\uDD1E\uDD80-\uDDF2]|\uD82B[\uDFF0-\uDFFF]|\uD82C[\uDC00-\uDEFB]|\uD83C[\uDE00-\uDEFF]|\uD869[\uDC00-\uDEDF\uDF00-\uDFFF]|\uD86E[\uDC00-\uDC1D\uDC20-\uDFFF]|\uD873[\uDC00-\uDEAD\uDEB0-\uDFFF]|\uD87A[\uDC00-\uDFE0\uDFF0-\uDFFF]|\uD87B[\uDC00-\uDE5D]|\uD87E[\uDC00-\uDE1D]|\uD884[\uDC00-\uDF4A\uDF50-\uDFFF]|\uD88D[\uDC00-\uDC79]/gim.test(String.fromCodePoint(codePoint));
}

/**
 * Returns true if the given Unicode codepoint identifies a character with
 * upright orientation.
 *
 * A character has upright orientation if it is drawn upright (unrotated)
 * whether the line is oriented horizontally or vertically, even if both
 * adjacent characters can be rotated. For example, a Chinese character is
 * always drawn upright. An uprightly oriented character causes an adjacent
 * “neutral” character to be drawn upright as well.
 */
export function codePointHasUprightVerticalOrientation(codePoint: number): boolean {
    return /[\u02EA\u02EB\u1100-\u11FF\u1401-\u167F\u18B0-\u18FF\u2065\u20DD-\u20E0\u20E2-\u20E4\u2B97\u2BF0-\u2BFF\u2E50\u2E51\u2E80-\u3007\u3012\u3013\u3020-\u302F\u3031-\u30FB\u30FD-\uA4CF\uA960-\uA97F\uAC00-\uD7FF\uF900-\uFAFF\uFE10-\uFE1F\uFE30-\uFE48\uFE50-\uFE57\uFE5F-\uFE62\uFE67-\uFE6F\uFF00-\uFF07\uFF0A-\uFF0C\uFF0E-\uFF19\uFF1F-\uFF3A\uFF3C\uFF3E\uFF40-\uFF5A\uFFE0-\uFFE2\uFFE4-\uFFE7\uFFF0-\uFFF8]|\uD802[\uDD80-\uDD9F]|\uD805[\uDD80-\uDDFF]|\uD806[\uDE00-\uDEBF]|[\uD80C-\uD810\uD81C-\uD822\uD83C\uD83D\uD840-\uD87E\uD880-\uD8BE][\uDC00-\uDFFF]|\uD811[\uDC00-\uDE7F]|\uD81B[\uDFE0-\uDFFF]|\uD823[\uDC00-\uDDFF]|\uD82B[\uDFF0-\uDFFF]|\uD82C[\uDC00-\uDEFF]|\uD833[\uDEC0-\uDFCF]|\uD834[\uDC00-\uDDFF\uDEE0-\uDF7F]|\uD836[\uDC00-\uDEAF]|\uD83E[\uDD00-\uDEFF]|[\uD87F\uD8BF][\uDC00-\uDFFD]/gim.test(String.fromCodePoint(codePoint));
}

/**
 * Returns true if the given Unicode codepoint identifies a character with
 * neutral orientation.
 *
 * A character has neutral orientation if it may be drawn rotated or unrotated
 * when the line is oriented vertically, depending on the orientation of the
 * adjacent characters. For example, along a vertically oriented line, the
 * vulgar fraction ½ is drawn upright among Chinese characters but rotated among
 * Latin letters. A neutrally oriented character does not influence whether an
 * adjacent character is drawn upright or rotated.
 */
export function codePointHasNeutralVerticalOrientation(codePoint: number): boolean {
    return /[\xA7\xA9\xAE\xB1\xBC-\xBE\xD7\xF7\u2016\u2020\u2021\u2030\u2031\u203B\u203C\u2042\u2047-\u2049\u2051\u2100-\u218F\u221E\u2234\u2235\u2300-\u2307\u230C-\u231F\u2324-\u2328\u232B\u237D-\u239A\u23BE-\u23CD\u23CF\u23D1-\u23DB\u23E2-\u2422\u2424-\u24FF\u25A0-\u2619\u2620-\u2767\u2776-\u2793\u2B12-\u2B2F\u2B50-\u2B59\u2BB8-\u2BEB\u3000-\u303F\u30A0-\u30FF\uE000-\uF8FF\uFE30-\uFE6F\uFF00-\uFFEF\uFFFC\uFFFD]|[\uDB80-\uDBFF][\uDC00-\uDFFF]/gim.test(String.fromCodePoint(codePoint));
}

/**
 * Returns whether the text could hold a grapheme cluster of more than one codepoint, and so is worth
 * segmenting. A negative answer means every codepoint of it stands alone.
 */
export function textCanContainGraphemeClusters(text: string): boolean {
    return /[\r\u0300-\u036F\u0483-\u0489\u0591-\u05BD\u05BF\u05C1\u05C2\u05C4\u05C5\u05C7\u0600-\u0605\u0610-\u061A\u064B-\u065F\u0670\u06D6-\u06DD\u06DF-\u06E4\u06E7\u06E8\u06EA-\u06ED\u070F-\u074A\u074D-\u074F\u07A6-\u07B0\u07CA-\u07F5\u07FA\u07FD\u0816-\u0819\u081B-\u0823\u0825-\u0827\u0829-\u082D\u0840-\u085B\u0860-\u086A\u0890\u0891\u0897-\u089F\u08CA-\u0903\u093A-\u093C\u093E-\u094F\u0951-\u0957\u0962\u0963\u0981-\u0983\u09BC\u09BE-\u09C4\u09C7\u09C8\u09CB-\u09CD\u09D7\u09E2\u09E3\u09FE\u0A01-\u0A03\u0A3C\u0A3E-\u0A42\u0A47\u0A48\u0A4B-\u0A4D\u0A51\u0A70\u0A71\u0A75\u0A81-\u0A83\u0ABC\u0ABE-\u0AC5\u0AC7-\u0AC9\u0ACB-\u0ACD\u0AE2\u0AE3\u0AFA-\u0AFF\u0B01-\u0B03\u0B3C\u0B3E-\u0B44\u0B47\u0B48\u0B4B-\u0B4D\u0B55-\u0B57\u0B62\u0B63\u0B82\u0BBE-\u0BC2\u0BC6-\u0BC8\u0BCA-\u0BCD\u0BD7\u0C00-\u0C04\u0C3C\u0C3E-\u0C44\u0C46-\u0C48\u0C4A-\u0C4D\u0C55\u0C56\u0C62\u0C63\u0C81-\u0C83\u0CBC\u0CBE-\u0CC4\u0CC6-\u0CC8\u0CCA-\u0CCD\u0CD5\u0CD6\u0CE2\u0CE3\u0CF3\u0D00-\u0D03\u0D3B\u0D3C\u0D3E-\u0D44\u0D46-\u0D48\u0D4A-\u0D4E\u0D57\u0D62\u0D63\u0D81-\u0D83\u0DCA\u0DCF-\u0DD4\u0DD6\u0DD8-\u0DDF\u0DF2\u0DF3\u0E31\u0E33-\u0E3A\u0E47-\u0E4E\u0EB1\u0EB3-\u0EBC\u0EC8-\u0ECE\u0F18\u0F19\u0F35\u0F37\u0F39\u0F3E\u0F3F\u0F71-\u0F84\u0F86\u0F87\u0F8D-\u0F97\u0F99-\u0FBC\u0FC6\u102D-\u1037\u1039-\u103E\u1056-\u1059\u105E-\u1060\u1071-\u1074\u1082\u1084-\u1086\u108D\u109D\u1100-\u11FF\u135D-\u135F\u1712-\u1715\u1732-\u1734\u1752\u1753\u1772\u1773\u17B4-\u17D3\u17DD\u180B-\u180F\u1820-\u1878\u1880-\u18AA\u1920-\u192B\u1930-\u193B\u1A17-\u1A1B\u1A55-\u1A5E\u1A60\u1A62\u1A65-\u1A7C\u1A7F\u1AB0-\u1ADD\u1AE0-\u1AEB\u1B00-\u1B04\u1B34-\u1B44\u1B6B-\u1B73\u1B80-\u1B82\u1BA1-\u1BAD\u1BE6-\u1BF3\u1C24-\u1C37\u1CD0-\u1CD2\u1CD4-\u1CE8\u1CED\u1CF4\u1CF7-\u1CF9\u1DC0-\u1DFF\u200C\u200D\u20D0-\u20F0\u2CEF-\u2CF1\u2D7F\u2DE0-\u2DFF\u302A-\u302F\u3099\u309A\uA66F-\uA672\uA674-\uA67D\uA69E\uA69F\uA6F0\uA6F1\uA802\uA806\uA80B\uA823-\uA827\uA82C\uA840-\uA873\uA880\uA881\uA8B4-\uA8C5\uA8E0-\uA8F1\uA8FF\uA926-\uA92D\uA947-\uA953\uA960-\uA97C\uA980-\uA983\uA9B3-\uA9C0\uA9E5\uAA29-\uAA36\uAA43\uAA4C\uAA4D\uAA7C\uAAB0\uAAB2-\uAAB4\uAAB7\uAAB8\uAABE\uAABF\uAAC1\uAAEB-\uAAEF\uAAF5\uAAF6\uABE3-\uABEA\uABEC\uABED\uAC00-\uD7A3\uD7B0-\uD7C6\uD7CB-\uD7FB\uFB1E\uFE00-\uFE0F\uFE20-\uFE2F\uFF9E\uFF9F]|\uD800[\uDDFD\uDEE0\uDF76-\uDF7A]|\uD802[\uDE01-\uDE03\uDE05\uDE06\uDE0C-\uDE0F\uDE38-\uDE3A\uDE3F\uDEC0-\uDEC7\uDEC9-\uDEE6\uDF80-\uDF91]|\uD803[\uDD00-\uDD27\uDD69-\uDD6D\uDEAB\uDEAC\uDEFA-\uDEFF\uDF30-\uDF50\uDF70-\uDF85\uDFB0-\uDFC4]|\uD804[\uDC00-\uDC02\uDC38-\uDC46\uDC70\uDC73\uDC74\uDC7F-\uDC82\uDCB0-\uDCBA\uDCBD\uDCC2\uDCCD\uDD00-\uDD02\uDD27-\uDD34\uDD45\uDD46\uDD73\uDD80-\uDD82\uDDB3-\uDDC0\uDDC2\uDDC3\uDDC9-\uDDCC\uDDCE\uDDCF\uDE2C-\uDE37\uDE3E\uDE41\uDEDF-\uDEEA\uDF00-\uDF03\uDF3B\uDF3C\uDF3E-\uDF44\uDF47\uDF48\uDF4B-\uDF4D\uDF57\uDF62\uDF63\uDF66-\uDF6C\uDF70-\uDF74\uDFB8-\uDFC0\uDFC2\uDFC5\uDFC7-\uDFCA\uDFCC-\uDFD2\uDFE1\uDFE2]|\uD805[\uDC35-\uDC46\uDC5E\uDCB0-\uDCC3\uDDAF-\uDDB5\uDDB8-\uDDC0\uDDDC\uDDDD\uDE30-\uDE40\uDEAB-\uDEB7\uDF1D-\uDF1F\uDF22-\uDF2B]|\uD806[\uDC2C-\uDC3A\uDD30-\uDD35\uDD37\uDD38\uDD3B-\uDD43\uDDD1-\uDDD7\uDDDA-\uDDE0\uDDE4\uDE01-\uDE0A\uDE33-\uDE39\uDE3B-\uDE3E\uDE47\uDE51-\uDE5B\uDE84-\uDE99\uDF60-\uDF67]|\uD807[\uDC2F-\uDC36\uDC38-\uDC3F\uDC92-\uDCA7\uDCA9-\uDCB6\uDD31-\uDD36\uDD3A\uDD3C\uDD3D\uDD3F-\uDD47\uDD8A-\uDD8E\uDD90\uDD91\uDD93-\uDD97\uDEF3-\uDEF6\uDF00-\uDF03\uDF34-\uDF3A\uDF3E-\uDF42\uDF5A]|\uD80D[\uDC40\uDC47-\uDC55]|\uD818[\uDD1E-\uDD2F]|\uD81A[\uDEF0-\uDEF4\uDF30-\uDF36]|\uD81B[\uDD63\uDD67-\uDD6A\uDF4F\uDF51-\uDF87\uDF8F-\uDF92\uDFE4\uDFF0\uDFF1]|\uD82F[\uDC00-\uDC6A\uDC70-\uDC7C\uDC80-\uDC88\uDC90-\uDC99\uDC9D\uDC9E]|\uD833[\uDF00-\uDF2D\uDF30-\uDF46]|\uD834[\uDD65-\uDD69\uDD6D-\uDD72\uDD7B-\uDD82\uDD85-\uDD8B\uDDAA-\uDDAD\uDE42-\uDE44]|\uD836[\uDE00-\uDE36\uDE3B-\uDE6C\uDE75\uDE84\uDE9B-\uDE9F\uDEA1-\uDEAF]|\uD838[\uDC00-\uDC06\uDC08-\uDC18\uDC1B-\uDC21\uDC23\uDC24\uDC26-\uDC2A\uDC8F\uDD30-\uDD36\uDEAE\uDEEC-\uDEEF]|\uD839[\uDCEC-\uDCEF\uDDEE\uDDEF\uDEE3\uDEE6\uDEEE\uDEEF\uDEF5]|\uD83A[\uDCD0-\uDCD6\uDD00-\uDD4B]|\uD83C[\uDDE6-\uDDFF\uDFFB-\uDFFF]|\uDB40[\uDC20-\uDC7F\uDD00-\uDDEF]/.test(text);
}

/**
 * Returns whether the given codepoint belongs to a script that does not put spaces between words,
 * and so can only be wrapped by asking the word segmenter where its words are.
 */
export function codePointIsWrittenWithoutSpaces(codePoint: number): boolean {
    return /[\u0E01-\u0E3A\u0E40-\u0E4E\u0E81\u0E82\u0E84\u0E86-\u0E8A\u0E8C-\u0EA3\u0EA5\u0EA7-\u0EBD\u0EC0-\u0EC4\u0EC6\u0EC8-\u0ECE\u0EDC-\u0EDF\u0F00-\u0F47\u0F49-\u0F6C\u0F71-\u0F97\u0F99-\u0FBC\u0FBE-\u0FCC\u0FCE-\u0FD4\u0FD9\u0FDA\u1000-\u103F\u1050-\u108F\u109A-\u109F\u1780-\u17D3\u17D7\u17DC\u17DD\u1950-\u196D\u1970-\u1974\u1980-\u19AB\u19B0-\u19C9\u19DE\u19DF\u1A20-\u1A5E\u1A60-\u1A7C\u1AA0-\u1AAD\u1B00-\u1B4C\u1B4E-\u1B7F\uA980-\uA9CD\uA9D0-\uA9D9\uA9DE-\uA9EF\uA9FA-\uA9FE\uAA60-\uAAC2\uAADB-\uAADF]|\uD805[\uDF00-\uDF1A\uDF1D-\uDF2B\uDF3A\uDF3B\uDF3F-\uDF46]/gim.test(String.fromCodePoint(codePoint));
}

const JOINS_UP = '(?:[\u070F-\u074A\u074D-\u074F\u07CA-\u07F5\u07FA\u07FD\u0840-\u085B\u0860-\u086A\u180B-\u180F\u1820-\u1878\u1880-\u18AA\uA840-\uA873]|\uD802[\uDEC0-\uDEC7\uDEC9-\uDEE6\uDF80-\uDF91]|\uD803[\uDD00-\uDD27\uDF30-\uDF50\uDF70-\uDF85\uDFB0-\uDFC4]|\uD82F[\uDC00-\uDC6A\uDC70-\uDC7C\uDC80-\uDC88\uDC90-\uDC99\uDC9D\uDC9E]|\uD83A[\uDD00-\uDD4B])';
const JOINS_UP_AT_END = new RegExp(`${JOINS_UP}$`);
const JOINS_UP_AT_START = new RegExp(`^${JOINS_UP}`);

/**
 * Returns whether two grapheme clusters found by `Intl.Segmenter` are really one unit of writing,
 * and so have to be measured and drawn as a whole.
 *
 * The segmenter follows the tailored rules CLDR uses for stepping a cursor through text, which put a
 * boundary after an invisible stacker and before a spacing mark. Laying text out wants the untailored
 * rules of UAX #29 instead: `လ`, `ာ` and `း` are one Burmese syllable, not three.
 *
 * Scenic Roads: a virama that writes its consonant joined to the next letter joins the two as well
 * (Kannada `ಬ್ಬ`, Gurmukhi `ਮ੍ਰ`), in every script that has one and whatever the browser's ICU
 * knows; Tamil's pulli joins only in its two conjuncts, `க்ஷ` and `ஸ்ரீ`. And a run of letters of
 * a script written joined up and drawn from a font on the page (Mongolian, Syriac, N'Ko: not
 * Arabic, joined through its presentation forms) is one unit, the word drawn whole.
 */
export function canCombineGraphemes(former: string, latter: string): boolean {
    if (JOINS_UP_AT_END.test(former) && JOINS_UP_AT_START.test(latter)) return true;
    return /(?:[\u1039\u17D2\u1A60\u1BAB\u200D\uAAF6]|\uD802\uDE3F|\uD804[\uDD33\uDFD0]|\uD806[\uDD3E\uDE47\uDE99]|\uD807[\uDD45\uDD97\uDF42])$/.test(former) || /^\p{gc=Mc}/u.test(latter) ||
        (/(?:[\u094D\u09CD\u0A4D\u0ACD\u0B4D\u0C4D\u0CCD\u0D4D\u1B44\uA806\uA8C4\uA9C0]|\uD804[\uDC46\uDCB9\uDDC0\uDE35\uDF4D]|\uD805[\uDC42\uDCC2\uDDBF\uDE3F\uDEB6]|\uD806[\uDC39\uDDE0]|\uD807\uDC3F)$/.test(former) && /^\p{L}/u.test(latter)) ||
        (former.endsWith('\u0B95\u0BCD') && latter.startsWith('\u0BB7')) ||
        (/[\u0BB6\u0BB8]\u0BCD$/.test(former) && latter.startsWith('\u0BB0\u0BC0'));
}

/**
 * Scenic Roads: returns whether the given codepoint belongs to a script that only a text engine can
 * write (vowel signs attached to consonants, or letters joined up that this library does not join
 * itself), so that its text is drawn from a font on the page a cluster at a time, never fetched a
 * codepoint at a time.
 */
export function codePointNeedsShaping(codePoint: number): boolean {
    return /[\u0700-\u070D\u070F-\u074A\u074D-\u074F\u07C0-\u07FA\u07FD-\u07FF\u0840-\u085B\u085E\u0860-\u086A\u0900-\u0950\u0955-\u0963\u0966-\u0983\u0985-\u098C\u098F\u0990\u0993-\u09A8\u09AA-\u09B0\u09B2\u09B6-\u09B9\u09BC-\u09C4\u09C7\u09C8\u09CB-\u09CE\u09D7\u09DC\u09DD\u09DF-\u09E3\u09E6-\u09FE\u0A01-\u0A03\u0A05-\u0A0A\u0A0F\u0A10\u0A13-\u0A28\u0A2A-\u0A30\u0A32\u0A33\u0A35\u0A36\u0A38\u0A39\u0A3C\u0A3E-\u0A42\u0A47\u0A48\u0A4B-\u0A4D\u0A51\u0A59-\u0A5C\u0A5E\u0A66-\u0A76\u0A81-\u0A83\u0A85-\u0A8D\u0A8F-\u0A91\u0A93-\u0AA8\u0AAA-\u0AB0\u0AB2\u0AB3\u0AB5-\u0AB9\u0ABC-\u0AC5\u0AC7-\u0AC9\u0ACB-\u0ACD\u0AD0\u0AE0-\u0AE3\u0AE6-\u0AF1\u0AF9-\u0AFF\u0B01-\u0B03\u0B05-\u0B0C\u0B0F\u0B10\u0B13-\u0B28\u0B2A-\u0B30\u0B32\u0B33\u0B35-\u0B39\u0B3C-\u0B44\u0B47\u0B48\u0B4B-\u0B4D\u0B55-\u0B57\u0B5C\u0B5D\u0B5F-\u0B63\u0B66-\u0B77\u0B82\u0B83\u0B85-\u0B8A\u0B8E-\u0B90\u0B92-\u0B95\u0B99\u0B9A\u0B9C\u0B9E\u0B9F\u0BA3\u0BA4\u0BA8-\u0BAA\u0BAE-\u0BB9\u0BBE-\u0BC2\u0BC6-\u0BC8\u0BCA-\u0BCD\u0BD0\u0BD7\u0BE6-\u0BFA\u0C00-\u0C0C\u0C0E-\u0C10\u0C12-\u0C28\u0C2A-\u0C39\u0C3C-\u0C44\u0C46-\u0C48\u0C4A-\u0C4D\u0C55\u0C56\u0C58-\u0C5A\u0C5C\u0C5D\u0C60-\u0C63\u0C66-\u0C6F\u0C77-\u0C8C\u0C8E-\u0C90\u0C92-\u0CA8\u0CAA-\u0CB3\u0CB5-\u0CB9\u0CBC-\u0CC4\u0CC6-\u0CC8\u0CCA-\u0CCD\u0CD5\u0CD6\u0CDC-\u0CDE\u0CE0-\u0CE3\u0CE6-\u0CEF\u0CF1-\u0CF3\u0D00-\u0D0C\u0D0E-\u0D10\u0D12-\u0D44\u0D46-\u0D48\u0D4A-\u0D4F\u0D54-\u0D63\u0D66-\u0D7F\u0D81-\u0D83\u0D85-\u0D96\u0D9A-\u0DB1\u0DB3-\u0DBB\u0DBD\u0DC0-\u0DC6\u0DCA\u0DCF-\u0DD4\u0DD6\u0DD8-\u0DDF\u0DE6-\u0DEF\u0DF2-\u0DF4\u0E01-\u0E3A\u0E40-\u0E5B\u0E81\u0E82\u0E84\u0E86-\u0E8A\u0E8C-\u0EA3\u0EA5\u0EA7-\u0EBD\u0EC0-\u0EC4\u0EC6\u0EC8-\u0ECE\u0ED0-\u0ED9\u0EDC-\u0EDF\u0F00-\u0F47\u0F49-\u0F6C\u0F71-\u0F97\u0F99-\u0FBC\u0FBE-\u0FCC\u0FCE-\u0FD4\u0FD9\u0FDA\u1000-\u109F\u1700-\u1715\u171F-\u1734\u1740-\u1753\u1760-\u176C\u176E-\u1770\u1772\u1773\u1780-\u17DD\u17E0-\u17E9\u17F0-\u17F9\u1800\u1801\u1804\u1806-\u1819\u1820-\u1878\u1880-\u18AA\u1900-\u191E\u1920-\u192B\u1930-\u193B\u1940\u1944-\u194F\u1980-\u19AB\u19B0-\u19C9\u19D0-\u19DA\u19DE-\u1A1B\u1A1E-\u1A5E\u1A60-\u1A7C\u1A7F-\u1A89\u1A90-\u1A99\u1AA0-\u1AAD\u1B00-\u1B4C\u1B4E-\u1BF3\u1BFC-\u1C37\u1C3B-\u1C49\u1C4D-\u1C4F\u1CC0-\u1CC7\uA800-\uA82C\uA840-\uA877\uA880-\uA8C5\uA8CE-\uA8D9\uA8E0-\uA8FF\uA930-\uA953\uA95F\uA980-\uA9CD\uA9D0-\uA9D9\uA9DE-\uA9FE\uAA00-\uAA36\uAA40-\uAA4D\uAA50-\uAA59\uAA5C-\uAAC2\uAADB-\uAAF6\uABC0-\uABED\uABF0-\uABF9]|\uD802[\uDE00-\uDE03\uDE05\uDE06\uDE0C-\uDE13\uDE15-\uDE17\uDE19-\uDE35\uDE38-\uDE3A\uDE3F-\uDE48\uDE50-\uDE58\uDEC0-\uDEE6\uDEEB-\uDEF6\uDF80-\uDF91\uDF99-\uDF9C\uDFA9-\uDFAF]|\uD803[\uDD00-\uDD27\uDD30-\uDD39\uDF30-\uDF59\uDF70-\uDF89\uDFB0-\uDFCB]|\uD804[\uDC00-\uDC4D\uDC52-\uDC75\uDC7F-\uDCC2\uDCCD\uDD00-\uDD34\uDD36-\uDD47\uDD80-\uDDDF\uDDE1-\uDDF4\uDE00-\uDE11\uDE13-\uDE41\uDEB0-\uDEEA\uDEF0-\uDEF9\uDF00-\uDF03\uDF05-\uDF0C\uDF0F\uDF10\uDF13-\uDF28\uDF2A-\uDF30\uDF32\uDF33\uDF35-\uDF39\uDF3C-\uDF44\uDF47\uDF48\uDF4B-\uDF4D\uDF50\uDF57\uDF5D-\uDF63\uDF66-\uDF6C\uDF70-\uDF74\uDF80-\uDF89\uDF8B\uDF8E\uDF90-\uDFB5\uDFB7-\uDFC0\uDFC2\uDFC5\uDFC7-\uDFCA\uDFCC-\uDFD5\uDFD7\uDFD8\uDFE1\uDFE2]|\uD805[\uDC00-\uDC5B\uDC5D-\uDC61\uDC80-\uDCC7\uDCD0-\uDCD9\uDD80-\uDDB5\uDDB8-\uDDDD\uDE00-\uDE44\uDE50-\uDE59\uDE60-\uDE6C\uDE80-\uDEB9\uDEC0-\uDEC9\uDED0-\uDEE3\uDF00-\uDF1A\uDF1D-\uDF2B\uDF30-\uDF46]|\uD806[\uDC00-\uDC3B\uDD00-\uDD06\uDD09\uDD0C-\uDD13\uDD15\uDD16\uDD18-\uDD35\uDD37\uDD38\uDD3B-\uDD46\uDD50-\uDD59\uDDA0-\uDDA7\uDDAA-\uDDD7\uDDDA-\uDDE4\uDE00-\uDE47\uDE50-\uDEA2\uDF00-\uDF09\uDF60-\uDF67]|\uD807[\uDC00-\uDC08\uDC0A-\uDC36\uDC38-\uDC45\uDC50-\uDC6C\uDC70-\uDC8F\uDC92-\uDCA7\uDCA9-\uDCB6\uDD00-\uDD06\uDD08\uDD09\uDD0B-\uDD36\uDD3A\uDD3C\uDD3D\uDD3F-\uDD47\uDD50-\uDD59\uDD60-\uDD65\uDD67\uDD68\uDD6A-\uDD8E\uDD90\uDD91\uDD93-\uDD98\uDDA0-\uDDA9\uDEE0-\uDEF8\uDF00-\uDF10\uDF12-\uDF3A\uDF3E-\uDF5A\uDFC0-\uDFF1\uDFFF]|\uD818[\uDD00-\uDD39]|\uD81B[\uDD40-\uDD79]|\uD82F[\uDC00-\uDC6A\uDC70-\uDC7C\uDC80-\uDC88\uDC90-\uDC99\uDC9C-\uDC9F]|\uD83A[\uDD00-\uDD4B\uDD50-\uDD59\uDD5E\uDD5F]/.test(String.fromCodePoint(codePoint));
}

/**
 * The joining type of each Arabic character, as `start,length` code point ranges delta-encoded in
 * base 36, one entry per type. Anything absent from all of them is non-joining.
 */
export const ENCODED_JOINING_TYPES: Record<string, string> = {
    C: '18g,1;g2,3;4nb,1',
    D: '17k,1;5,1;1,1;1,5;4,d;1,7;1,2;z,2;8,g;i,12;1,2;9,1;1,1;1,2;14,3;2,1;28,9;3,f;2,4;1,1;2,3;2,6;7a,1;2,5;1,1;g,a;5,2;2,6;1,f',
    R: '17m,4;1,1;1,1;5,4;l,1;14,3;1,3;g,i;12,1;2,9;1,1;1,1;2,2;1,1;o,2;2x,3;f,2;4,1;1,2;3,2;6u,j;b,1;r,3;1,1;2,2;6,1',
    T: '174,b;1,1;1a,l;g,1;2t,7;2,6;2,2;1,4;bt,9;16,o;1,t;1clb,1',
};

/**
 * Each Arabic letter's Presentation Forms code points, as `[isolated, final, initial, medial]`.
 * A shape the letter is not written in is 0.
 */
export const PRESENTATION_FORMS: Record<number, [number, number, number, number]> = {
    1569: [65152, 0, 0, 0],
    1570: [65153, 65154, 0, 0],
    1571: [65155, 65156, 0, 0],
    1572: [65157, 65158, 0, 0],
    1573: [65159, 65160, 0, 0],
    1574: [65161, 65162, 65163, 65164],
    1575: [65165, 65166, 0, 0],
    1576: [65167, 65168, 65169, 65170],
    1577: [65171, 65172, 0, 0],
    1578: [65173, 65174, 65175, 65176],
    1579: [65177, 65178, 65179, 65180],
    1580: [65181, 65182, 65183, 65184],
    1581: [65185, 65186, 65187, 65188],
    1582: [65189, 65190, 65191, 65192],
    1583: [65193, 65194, 0, 0],
    1584: [65195, 65196, 0, 0],
    1585: [65197, 65198, 0, 0],
    1586: [65199, 65200, 0, 0],
    1587: [65201, 65202, 65203, 65204],
    1588: [65205, 65206, 65207, 65208],
    1589: [65209, 65210, 65211, 65212],
    1590: [65213, 65214, 65215, 65216],
    1591: [65217, 65218, 65219, 65220],
    1592: [65221, 65222, 65223, 65224],
    1593: [65225, 65226, 65227, 65228],
    1594: [65229, 65230, 65231, 65232],
    1601: [65233, 65234, 65235, 65236],
    1602: [65237, 65238, 65239, 65240],
    1603: [65241, 65242, 65243, 65244],
    1604: [65245, 65246, 65247, 65248],
    1605: [65249, 65250, 65251, 65252],
    1606: [65253, 65254, 65255, 65256],
    1607: [65257, 65258, 65259, 65260],
    1608: [65261, 65262, 0, 0],
    1609: [65263, 65264, 64488, 64489],
    1610: [65265, 65266, 65267, 65268],
    1611: [65136, 0, 0, 65137],
    1612: [65138, 0, 0, 0],
    1613: [65140, 0, 0, 0],
    1614: [65142, 0, 0, 65143],
    1615: [65144, 0, 0, 65145],
    1616: [65146, 0, 0, 65147],
    1617: [65148, 0, 0, 65149],
    1618: [65150, 0, 0, 65151],
    1649: [64336, 64337, 0, 0],
    1655: [64477, 0, 0, 0],
    1657: [64358, 64359, 64360, 64361],
    1658: [64350, 64351, 64352, 64353],
    1659: [64338, 64339, 64340, 64341],
    1662: [64342, 64343, 64344, 64345],
    1663: [64354, 64355, 64356, 64357],
    1664: [64346, 64347, 64348, 64349],
    1667: [64374, 64375, 64376, 64377],
    1668: [64370, 64371, 64372, 64373],
    1670: [64378, 64379, 64380, 64381],
    1671: [64382, 64383, 64384, 64385],
    1672: [64392, 64393, 0, 0],
    1676: [64388, 64389, 0, 0],
    1677: [64386, 64387, 0, 0],
    1678: [64390, 64391, 0, 0],
    1681: [64396, 64397, 0, 0],
    1688: [64394, 64395, 0, 0],
    1700: [64362, 64363, 64364, 64365],
    1702: [64366, 64367, 64368, 64369],
    1705: [64398, 64399, 64400, 64401],
    1709: [64467, 64468, 64469, 64470],
    1711: [64402, 64403, 64404, 64405],
    1713: [64410, 64411, 64412, 64413],
    1715: [64406, 64407, 64408, 64409],
    1722: [64414, 64415, 0, 0],
    1723: [64416, 64417, 64418, 64419],
    1726: [64426, 64427, 64428, 64429],
    1728: [64420, 64421, 0, 0],
    1729: [64422, 64423, 64424, 64425],
    1733: [64480, 64481, 0, 0],
    1734: [64473, 64474, 0, 0],
    1735: [64471, 64472, 0, 0],
    1736: [64475, 64476, 0, 0],
    1737: [64482, 64483, 0, 0],
    1739: [64478, 64479, 0, 0],
    1740: [64508, 64509, 64510, 64511],
    1744: [64484, 64485, 64486, 64487],
    1746: [64430, 64431, 0, 0],
    1747: [64432, 64433, 0, 0],
};

/** The lam-alef ligatures, keyed by the pair of letters they replace, as `[isolated, final]`. */
export const LIGATURES: Record<string, [number, number]> = {
    '\u0644\u0622': [65269, 65270],
    '\u0644\u0623': [65271, 65272],
    '\u0644\u0625': [65273, 65274],
    '\u0644\u0627': [65275, 65276],
};
