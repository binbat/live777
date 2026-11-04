# Live777 diagnostics third-party notices

The diagnostics programs are built from Live777 and link to FFmpeg and its
enabled codec libraries. FFmpeg is configured with `--enable-gpl`; the FFmpeg
build and components including x264 and x265 are distributed under GPL terms,
not under Live777's MPL-2.0 license. Other libraries retain their own licenses.

This directory contains the common license texts and notices. The `packages/`
or `vcpkg/` subdirectory, where present, contains the copyright and license
notice shipped by each actual packaged dependency. The archive's
`third-party/copied-files.tsv` records the origin of every copied dynamic library.

The matching `live777-diagnostics-sources-<version>-<target>` release archive
contains the Live777 source snapshot, the exact FFmpeg source and build
configuration, and the corresponding dependency source downloads and package
recipes. Its `MANIFEST.tsv` identifies the binary archive by SHA-256 and its
`REBUILD.md` describes how to use the sources. The source archive is part of the
binary's corresponding-source offer; it is not merely a license disclaimer.

The exact enabled components vary by target. Refer to the dependency notices
and the source archive rather than treating this summary as a complete license
list.

## Maintaining this directory

The texts are vendored so that packaging never depends on the runner's package
manager. Refresh them when FFmpeg or a codec is upgraded: `GPL-2.0.txt`,
`LGPL-2.1.txt`, `GPL-3.0.txt`, `LGPL-3.0.txt` and `Apache-2.0.txt` are the
canonical texts, `FFmpeg-LICENSE.md` comes from the FFmpeg tree, and
`MSVC-RUNTIME-NOTICE.md` covers the Windows runtime DLLs. Packaging checks that
the required notices for each target are present and non-empty; review their
content whenever upstream versions change.
