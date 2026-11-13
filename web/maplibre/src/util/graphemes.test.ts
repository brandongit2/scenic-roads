import {describe, expect, test} from 'vitest';
import {isCluster, supportsGraphemeSegmentation, toGraphemes, wordBoundaries} from './graphemes.ts';
import {canCombineGraphemes, codePointNeedsShaping} from './unicode_properties.g.ts';

describe('toGraphemes', () => {
    test('leaves plain text one unit per character', () => {
        expect(toGraphemes('Tel Aviv')).toEqual(['T', 'e', 'l', ' ', 'A', 'v', 'i', 'v']);
    });

    test('keeps a letter and the combining mark written on it together', () => {
        expect(toGraphemes('é')).toEqual(['é']);
    });

    test('keeps a Hebrew letter and its vowel points together: one letter, three codepoints', () => {
        expect(toGraphemes('שְׁ')).toEqual(['שְׁ']);
        expect(toGraphemes('שְׁדֵ')).toEqual(['שְׁ', 'דֵ']);
    });

    test('keeps a Devanagari syllable together, vowel sign written before its consonant and all', () => {
        expect(toGraphemes('दिल्ली')).toEqual(['दि', 'ल्ली']);
    });

    test('keeps a Khmer syllable with a subscript consonant together', () => {
        expect(toGraphemes('ភ្នំ')).toEqual(['ភ្នំ']);
    });

    test('keeps a Burmese syllable together across the spacing-mark boundaries the segmenter tailors in', () => {
        expect(toGraphemes('ဘင်္ဂလားဒေ့ရှ်')).toEqual(['ဘ', 'င်္ဂ', 'လား', 'ဒေ့', 'ရှ်']);
    });

    describe('Scenic Roads: a virama that joins, and words written joined up', () => {
        test('keeps a conjunct together in the scripts the segmenter splits at the virama', () => {
            // Kannada: Hubballi, Srirangapatna. Gurmukhi: Amritsar, Chandigarh.
            expect(toGraphemes('ಹುಬ್ಬಳ್ಳಿ')).toEqual(['ಹು', 'ಬ್ಬ', 'ಳ್ಳಿ']);
            expect(toGraphemes('ಶ್ರೀರಂಗಪಟ್ಟಣ')).toEqual(['ಶ್ರೀ', 'ರಂ', 'ಗ', 'ಪ', 'ಟ್ಟ', 'ಣ']);
            expect(toGraphemes('ਅੰਮ੍ਰਿਤਸਰ')).toEqual(['ਅੰ', 'ਮ੍ਰਿ', 'ਤ', 'ਸ', 'ਰ']);
            expect(toGraphemes('ਚੰਡੀਗੜ੍ਹ')).toEqual(['ਚੰ', 'ਡੀ', 'ਗ', 'ੜ੍ਹ']);
        });

        test('keeps the conjuncts of the scripts the segmenter may or may not join, whatever it does', () => {
            // What a segmenter without Unicode's rule GB9c hands over: split after the virama.
            expect(canCombineGraphemes('ल्', 'ली')).toBe(true); // Devanagari
            expect(canCombineGraphemes('ট্', 'ট')).toBe(true); // Bengali
            expect(canCombineGraphemes('ન્', 'દ્')).toBe(true); // Gujarati
            expect(canCombineGraphemes('ଶ୍', 'ୱ')).toBe(true); // Odia
            expect(canCombineGraphemes('ట్', 'న')).toBe(true); // Telugu
            expect(canCombineGraphemes('ച്', 'ചി')).toBe(true); // Malayalam
            expect(toGraphemes('दिल्ली')).toEqual(['दि', 'ल्ली']);
            expect(toGraphemes('ভুবনেশ্বর চট্টগ্রাম')).toEqual(['ভু', 'ব', 'নে', 'শ্ব', 'র', ' ', 'চ', 'ট্ট', 'গ্রা', 'ম']);
        });

        test('leaves a virama that ends a word, or is kept from joining, alone', () => {
            expect(canCombineGraphemes('ದ್', ' ')).toBe(false); // Hyderabad, then a space
            expect(canCombineGraphemes('क्‌', 'ष')).toBe(false); // a zero-width non-joiner: no conjunct
            expect(toGraphemes('ಹೈದರಾಬಾದ್ ನಗರ')).toEqual(['ಹೈ', 'ದ', 'ರಾ', 'ಬಾ', 'ದ್', ' ', 'ನ', 'ಗ', 'ರ']);
        });

        test('joins Tamil only in its two conjuncts: the pulli elsewhere joins nothing', () => {
            expect(toGraphemes('ஸ்ரீரங்கம்')).toEqual(['ஸ்ரீ', 'ர', 'ங்', 'க', 'ம்']);
            expect(toGraphemes('மீனாக்ஷி')).toEqual(['மீ', 'னா', 'க்ஷி']);
            expect(toGraphemes('திருச்சிராப்பள்ளி')).toEqual(['தி', 'ரு', 'ச்', 'சி', 'ரா', 'ப்', 'ப', 'ள்', 'ளி']);
        });

        test('leaves Sinhala to its joiner: the al-lakuna alone joins nothing', () => {
            expect(toGraphemes('කෝට්ටේ')).toEqual(['කෝ', 'ට්', 'ටේ']);
            expect(toGraphemes('ශ්‍රී')).toEqual(['ශ්‍රී']);
        });

        test('takes a Mongolian word as one unit, its letters shaped by their neighbours', () => {
            expect(toGraphemes('ᠮᠣᠩᠭᠣᠯ ᠤᠯᠤᠰ')).toEqual(['ᠮᠣᠩᠭᠣᠯ', ' ', 'ᠤᠯᠤᠰ']);
            expect(toGraphemes('ܐܘܪܗܝ')).toEqual(['ܐܘܪܗܝ']); // Syriac
        });

        test('leaves Arabic, joined through its presentation forms, a letter at a time', () => {
            expect(toGraphemes('القاهرة')).toEqual([...'القاهرة']);
        });

        test('says which scripts are drawn from a font on the page', () => {
            for (const letter of 'दঢਅઅକசహಬമකกວལမភᠮܐᨠᬅ') expect(codePointNeedsShaping(letter.codePointAt(0))).toBe(true);
            for (const letter of 'aθжשمﻣԵთአ東あ한ᐃ1 -।') expect(codePointNeedsShaping(letter.codePointAt(0))).toBe(false);
        });
    });

    test('does not split a character outside the basic plane', () => {
        expect(toGraphemes('\u{30EDE}\u{30EDE}')).toEqual(['\u{30EDE}', '\u{30EDE}']);
    });

    test('handles the empty string', () => {
        expect(toGraphemes('')).toEqual([]);
    });
});

describe('isCluster', () => {
    test('is false for a single character, inside the basic plane or outside it', () => {
        expect(isCluster('a')).toBe(false);
        expect(isCluster(' ')).toBe(false);
        expect(isCluster('麵')).toBe(false);
        expect(isCluster('\u{30EDE}')).toBe(false);
    });

    test('is true for a character written with marks on it', () => {
        expect(isCluster('é')).toBe(true);
        expect(isCluster('שְׁ')).toBe(true);
        expect(isCluster('\u{30EDE}́')).toBe(true);
    });
});

test('the environment can segment graphemes, so that the tests above are not silently vacuous', () => {
    expect(supportsGraphemeSegmentation).toBe(true);
});

describe('wordBoundaries', () => {
    test('finds where each word of spaced text begins', () => {
        expect([...wordBoundaries('Tel Aviv')].sort((a, b) => a - b)).toEqual([0, 3, 4]);
    });

    test('finds the words of Thai, which spaces none of them, from the browser dictionary', () => {
        expect(wordBoundaries('ราชอาณาจักรไทย').has('ราชอาณาจักร'.length)).toBe(true);
    });

    test('offers a break at a newline, whether or not a carriage return precedes it', () => {
        expect(wordBoundaries('ab\ncd').has(3)).toBe(true);
        expect(wordBoundaries('ab\r\ncd').has(4)).toBe(true);
    });

    test('offers a break at a zero-width space, which some tilesets insert as a hint', () => {
        expect(wordBoundaries('三三\u200b三三').has(3)).toBe(true);
    });

    test('handles the empty string', () => {
        expect([...wordBoundaries('')]).toEqual([]);
    });
});
