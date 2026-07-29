type TimestampCandidate<TMatrix> = {
    matrix: TMatrix;
};

type TimestampDecodeResult = {
    content: string;
};

export function decodeTimestampCandidates<TMatrix>(
    candidates: Generator<TimestampCandidate<TMatrix>, void, boolean>,
    decode: (matrix: TMatrix) => TimestampDecodeResult,
): number | null {
    let current = candidates.next();

    while (!current.done) {
        try {
            const decoded = decode(current.value.matrix);
            const content = decoded.content;

            if (/^[1-9]\d*$/.test(content)) {
                const timestamp = Number(content);
                if (Number.isSafeInteger(timestamp)) {
                    return timestamp;
                }
            }
        } catch {
            // A detected region is only a candidate and may not be decodable.
        }

        current = candidates.next(false);
    }

    return null;
}
