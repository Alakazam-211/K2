# Third-party notices

K2 by Alakazam Labs. K2 itself is licensed under the MIT License (see
`LICENSE.md` in the source tree). This file holds the licence notices for
third-party code and data that ship with K2: the desktop app (macOS,
Windows and Linux), the server package (the `k2-daemon` .deb and the Arch
package) and the hosted web app.

This file is kept by hand. Part 4 is `NOTICE.md` from the source tree,
copied in unchanged; `scripts/check-third-party-notices.sh` fails when the
copy and `NOTICE.md` differ, and when a package or the web build leaves this
file out.

Contents

1. The PostgreSQL parser: libpg_query, through the pg_query crate
2. SQLite, through the rusqlite and libsqlite3-sys crates
3. libgit2 in earlier releases
4. NOTICE.md

## 1. The PostgreSQL parser: libpg_query (PostgreSQL License, BSD-3-Clause, and other permissive notices)

The K2 daemon parses SQL for Apps' database doors with the Rust crate
**pg_query 6.2.1**, which compiles in **libpg_query 17-6.2.5**, which in turn
carries parser source code from **PostgreSQL 17.7**, plus the vendored
**protobuf-c** and **xxHash** libraries. Its upstream sources:

- pg_query (Rust): https://github.com/pganalyze/pg_query.rs (tag `v6.2.1`)
- libpg_query: https://github.com/pganalyze/libpg_query (tag `17-6.2.5`)
- PostgreSQL: https://www.postgresql.org/ (tag `REL_17_7`)

The crate as published on crates.io does not include the PostgreSQL
`COPYRIGHT` file or libpg_query's and pg_query's `LICENSE` files, so the
three texts below are those files at the tags above. The other notices are
copied from the source files the crate compiles (paths are inside the crate,
`pg_query-6.2.1/`).

### PostgreSQL License (PostgreSQL's `COPYRIGHT` at `REL_17_7`)

The PostgreSQL source files in libpg_query carry the lines
"Portions Copyright (c) 1996-2024, PostgreSQL Global Development Group" and
"Portions Copyright (c) 1994, The Regents of the University of California"
(a few say 1994-5 or 1995).

```text
PostgreSQL Database Management System
(also known as Postgres, formerly known as Postgres95)

Portions Copyright (c) 1996-2025, PostgreSQL Global Development Group

Portions Copyright (c) 1994, The Regents of the University of California

Permission to use, copy, modify, and distribute this software and its
documentation for any purpose, without fee, and without a written agreement
is hereby granted, provided that the above copyright notice and this
paragraph and the following two paragraphs appear in all copies.

IN NO EVENT SHALL THE UNIVERSITY OF CALIFORNIA BE LIABLE TO ANY PARTY FOR
DIRECT, INDIRECT, SPECIAL, INCIDENTAL, OR CONSEQUENTIAL DAMAGES, INCLUDING
LOST PROFITS, ARISING OUT OF THE USE OF THIS SOFTWARE AND ITS
DOCUMENTATION, EVEN IF THE UNIVERSITY OF CALIFORNIA HAS BEEN ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.

THE UNIVERSITY OF CALIFORNIA SPECIFICALLY DISCLAIMS ANY WARRANTIES,
INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY
AND FITNESS FOR A PARTICULAR PURPOSE.  THE SOFTWARE PROVIDED HEREUNDER IS
ON AN "AS IS" BASIS, AND THE UNIVERSITY OF CALIFORNIA HAS NO OBLIGATIONS TO
PROVIDE MAINTENANCE, SUPPORT, UPDATES, ENHANCEMENTS, OR MODIFICATIONS.
```

### libpg_query (BSD-3-Clause; libpg_query's `LICENSE` at `17-6.2.5`)

```text
Copyright (c) 2015, Lukas Fittl <lukas@fittl.com>
Copyright (c) 2016-2023, Duboce Labs, Inc. (pganalyze) <team@pganalyze.com>
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this
list of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright notice,
this list of conditions and the following disclaimer in the documentation
and/or other materials provided with the distribution.

* Neither the name of pg_query nor the names of its contributors may be used
to endorse or promote products derived from this software without specific
prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.
```

### pg_query (MIT; pg_query.rs's `LICENSE` at `v6.2.1`)

```text
MIT License

Copyright (c) 2021 Paul Mason <paul@form1.co.nz>
Copyright (c) 2021-2023, Duboce Labs, Inc. (pganalyze) <team@pganalyze.com>
All rights reserved.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### Other notices in the code libpg_query compiles

#### protobuf-c (BSD-2-Clause), `libpg_query/vendor/protobuf-c/protobuf-c.h`

```text
/*
 * Copyright (c) 2008-2018, Dave Benson and the protobuf-c authors.
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions are
 * met:
 *
 *     * Redistributions of source code must retain the above copyright
 * notice, this list of conditions and the following disclaimer.
 *
 *     * Redistributions in binary form must reproduce the above
 * copyright notice, this list of conditions and the following disclaimer
 * in the documentation and/or other materials provided with the
 * distribution.
 *
 * THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
 * "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
 * LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
 * A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
 * OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
 * SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
 * LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
 * DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
 * THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
 * (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
 * OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 */
```

#### xxHash (BSD-2-Clause), `libpg_query/vendor/xxhash/xxhash.h`

```text
/*
 * xxHash - Extremely Fast Hash algorithm
 * Header File
 * Copyright (C) 2012-2020 Yann Collet
 *
 * BSD 2-Clause License (https://www.opensource.org/licenses/bsd-license.php)
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions are
 * met:
 *
 *    * Redistributions of source code must retain the above copyright
 *      notice, this list of conditions and the following disclaimer.
 *    * Redistributions in binary form must reproduce the above
 *      copyright notice, this list of conditions and the following disclaimer
 *      in the documentation and/or other materials provided with the
 *      distribution.
 *
 * THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
 * "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
 * LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
 * A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
 * OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
 * SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
 * LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
 * DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
 * THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
 * (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
 * OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 *
 * You can contact the author at:
 *   - xxHash homepage: https://www.xxhash.com
 *   - xxHash source repository: https://github.com/Cyan4973/xxHash
 */
```

#### PostgreSQL's snprintf (BSD-3-Clause), `libpg_query/src/postgres/src_port_snprintf.c`

```text
/*
 * Copyright (c) 1983, 1995, 1996 Eric P. Allman
 * Copyright (c) 1988, 1993
 *	The Regents of the University of California.  All rights reserved.
 * Portions Copyright (c) 1996-2024, PostgreSQL Global Development Group
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 * 1. Redistributions of source code must retain the above copyright
 *	  notice, this list of conditions and the following disclaimer.
 * 2. Redistributions in binary form must reproduce the above copyright
 *	  notice, this list of conditions and the following disclaimer in the
 *	  documentation and/or other materials provided with the distribution.
 * 3. Neither the name of the University nor the names of its contributors
 *	  may be used to endorse or promote products derived from this software
 *	  without specific prior written permission.
 *
 * THIS SOFTWARE IS PROVIDED BY THE REGENTS AND CONTRIBUTORS ``AS IS'' AND
 * ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
 * IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
 * ARE DISCLAIMED.  IN NO EVENT SHALL THE REGENTS OR CONTRIBUTORS BE LIABLE
 * FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
 * DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
 * OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
 * HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
 * LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
 * OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
 * SUCH DAMAGE.
 *
 * src/port/snprintf.c
 */
```

#### PostgreSQL's strlcpy, from OpenBSD (ISC), `libpg_query/src/postgres/src_port_strlcpy.c`

```text
/*
 * Copyright (c) 1998 Todd C. Miller <Todd.Miller@courtesan.com>
 *
 * Permission to use, copy, modify, and distribute this software for any
 * purpose with or without fee is hereby granted, provided that the above
 * copyright notice and this permission notice appear in all copies.
 *
 * THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
 * WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
 * MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
 * ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
 * WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
 * ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
 * OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
 */
```

#### PostgreSQL's sort template (BSD-3-Clause), `libpg_query/src/postgres/include/lib/sort_template.h`

```text
/*-
 * Copyright (c) 1992, 1993
 *	  The Regents of the University of California.  All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 * 1. Redistributions of source code must retain the above copyright
 *	  notice, this list of conditions and the following disclaimer.
 * 2. Redistributions in binary form must reproduce the above copyright
 *	  notice, this list of conditions and the following disclaimer in the
 *	  documentation and/or other materials provided with the distribution.
 * 3. Neither the name of the University nor the names of its contributors
 *	  may be used to endorse or promote products derived from this software
 *	  without specific prior written permission.
 *
 * THIS SOFTWARE IS PROVIDED BY THE REGENTS AND CONTRIBUTORS ``AS IS'' AND
 * ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
 * IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
 * ARE DISCLAIMED.  IN NO EVENT SHALL THE REGENTS OR CONTRIBUTORS BE LIABLE
 * FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
 * DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
 * OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
 * HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
 * LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
 * OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
 * SUCH DAMAGE.
 */
```

#### PostgreSQL's fasthash (MIT), `libpg_query/src/postgres/include/common/hashfn_unstable.h`

```text
/* The MIT License

   Copyright (C) 2012 Zilong Tan (eric.zltan@gmail.com)

   Permission is hereby granted, free of charge, to any person
   obtaining a copy of this software and associated documentation
   files (the "Software"), to deal in the Software without
   restriction, including without limitation the rights to use, copy,
   modify, merge, publish, distribute, sublicense, and/or sell copies
   of the Software, and to permit persons to whom the Software is
   furnished to do so, subject to the following conditions:

   The above copyright notice and this permission notice shall be
   included in all copies or substantial portions of the Software.

   THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
   EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
   MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
   NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS
   BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN
   ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
   CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
   SOFTWARE.
*/
```

#### PostgreSQL's regex header, Henry Spencer's package, `libpg_query/src/postgres/include/regex/regex.h`

```text
/*
 * regular expressions
 *
 * Copyright (c) 1998, 1999 Henry Spencer.  All rights reserved.
 *
 * Development of this software was funded, in part, by Cray Research Inc.,
 * UUNET Communications Services Inc., Sun Microsystems Inc., and Scriptics
 * Corporation, none of whom are responsible for the results.  The author
 * thanks all of them.
 *
 * Redistribution and use in source and binary forms -- with or without
 * modification -- are permitted for any purpose, provided that
 * redistributions in source form retain this entire copyright notice and
 * indicate the origin and nature of any modifications.
 *
 * I'd appreciate being given credit for this package in the documentation
 * of software which uses it, but that is not a requirement.
 *
 * THIS SOFTWARE IS PROVIDED ``AS IS'' AND ANY EXPRESS OR IMPLIED WARRANTIES,
 * INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY
 * AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED.  IN NO EVENT SHALL
 * HENRY SPENCER BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
 * EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
 * PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS;
 * OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
 * WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR
 * OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF
 * ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 *
 * src/include/regex/regex.h
 */
```

#### PostgreSQL's getopt header, `libpg_query/src/postgres/include/pg_getopt.h`

```text
/*
 * Postgres files that use getopt(3) always include this file.
 * We must cope with three different scenarios:
 * 1. We're using the platform's getopt(), and we should just import the
 *    appropriate declarations.
 * 2. The platform lacks getopt(), and we must declare everything.
 * 3. The platform has getopt(), but we're not using it because we don't
 *    like its behavior.  The declarations we make here must be compatible
 *    with both the platform's getopt() and our src/port/getopt.c.
 *
 * Portions Copyright (c) 1987, 1993, 1994
 * The Regents of the University of California.  All rights reserved.
 *
 * Portions Copyright (c) 2003-2024, PostgreSQL Global Development Group
 *
 * src/include/pg_getopt.h
 */
```

#### The Bison parser skeleton (GPL-2.0-or-later with the Bison exception)

PostgreSQL's two grammars (`libpg_query/src/postgres/src_backend_parser_gram.c`
and `src_pl_plpgsql_src_pl_gram.c`) were generated by Bison and carry the
notice and the special exception below, copied from
`src_backend_parser_gram.c`.

```text
/* Skeleton implementation for Bison's Yacc-like parsers in C

   Copyright (C) 1984, 1989, 1990, 2000, 2001, 2002, 2003, 2004, 2005, 2006
   Free Software Foundation, Inc.

   This program is free software; you can redistribute it and/or modify
   it under the terms of the GNU General Public License as published by
   the Free Software Foundation; either version 2, or (at your option)
   any later version.

   This program is distributed in the hope that it will be useful,
   but WITHOUT ANY WARRANTY; without even the implied warranty of
   MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
   GNU General Public License for more details.

   You should have received a copy of the GNU General Public License
   along with this program; if not, write to the Free Software
   Foundation, Inc., 51 Franklin Street, Fifth Floor,
   Boston, MA 02110-1301, USA.  */

/* As a special exception, you may create a larger work that contains
   part or all of the Bison parser skeleton and distribute that work
   under terms of your choice, so long as that work isn't itself a
   parser generator using the skeleton or a modified version thereof
   as a parser skeleton.  Alternatively, if you modify or redistribute
   the parser skeleton itself, you may (at your option) remove this
   special exception, which will cause the skeleton and the resulting
   Bison output files to be licensed under the GNU General Public
   License without this special exception.

   This special exception was added by the Free Software Foundation in
   version 2.2 of Bison.  */
```

## 2. SQLite (public domain), through rusqlite and libsqlite3-sys (MIT)

K2 keeps its local data in **SQLite 3.54.1**, compiled into the daemon (which
the desktop app carries too) and the compute node program from the
amalgamation that the
Rust crate **libsqlite3-sys 0.39.1** bundles (used through **rusqlite
0.41.1**). Upstream: https://sqlite.org/ ,
https://github.com/rusqlite/rusqlite .

### SQLite's dedication to the public domain (`libsqlite3-sys-0.39.1/sqlite3/sqlite3.h`)

SQLite's authors have dedicated its source code to the public domain
(https://sqlite.org/copyright.html). Each source file opens with this
notice in place of a licence:

```text
/*
** 2001-09-15
**
** The author disclaims copyright to this source code.  In place of
** a legal notice, here is a blessing:
**
**    May you do good and not evil.
**    May you find forgiveness for yourself and forgive others.
**    May you share freely, never taking more than you give.
**
```

### rusqlite and libsqlite3-sys (MIT; the `LICENSE` file both crates ship)

```text
Copyright (c) 2014 The rusqlite developers

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

## 3. libgit2 in earlier releases

<!-- libgit2-release-note: the text from here to the licence text is also the release notes' download-page text. -->

Earlier K2 releases included libgit2. Releases 0.9.0 through 0.46.17
(including those published under the product's earlier name) contain
**libgit2 1.8.1**, compiled in from the Rust crate **libgit2-sys
0.17.0+1.8.1**, which bundles that libgit2 release. K2 0.46.18 and later
releases do not include libgit2.

libgit2 is licensed under the GNU General Public License version 2 with a
linking exception. Its bundled xdiff code is licensed under the GNU Lesser
General Public License version 2.1, and its other bundled parts under the
licences its `COPYING` file lists. That file is copied below, unchanged.

Its source is available at:

- libgit2 1.8.1 upstream: https://github.com/libgit2/libgit2/tree/v1.8.1
  (archive: https://github.com/libgit2/libgit2/archive/refs/tags/v1.8.1.tar.gz)
- the exact source those releases were built from, as published on
  crates.io: https://crates.io/crates/libgit2-sys/0.17.0+1.8.1
  (archive: https://static.crates.io/crates/libgit2-sys/libgit2-sys-0.17.0+1.8.1.crate)

Written offer: for at least three years after the last release that
included libgit2, Alakazam Labs will give anyone who asks a complete,
machine-readable copy of the corresponding source code of libgit2 as it was
included in those releases, on a medium customarily used for software
interchange, for a charge no more than the cost of physically performing
the distribution. To ask, write to Alakazam Labs at support@k2.dev.

### libgit2 1.8.1 `COPYING` (`libgit2-sys-0.17.0+1.8.1/libgit2/COPYING`)

```text
 libgit2 is Copyright (C) the libgit2 contributors,
 unless otherwise stated. See the AUTHORS file for details.

 Note that the only valid version of the GPL as far as this project
 is concerned is _this_ particular version of the license (ie v2, not
 v2.2 or v3.x or whatever), unless explicitly otherwise stated.

----------------------------------------------------------------------

			LINKING EXCEPTION

 In addition to the permissions in the GNU General Public License,
 the authors give you unlimited permission to link the compiled
 version of this library into combinations with other programs,
 and to distribute those combinations without any restriction
 coming from the use of this file.  (The General Public License
 restrictions do apply in other respects; for example, they cover
 modification of the file, and distribution when not linked into
 a combined executable.)

----------------------------------------------------------------------

		    GNU GENERAL PUBLIC LICENSE
		       Version 2, June 1991

 Copyright (C) 1989, 1991 Free Software Foundation, Inc.
                       59 Temple Place, Suite 330, Boston, MA  02111-1307  USA
 Everyone is permitted to copy and distribute verbatim copies
 of this license document, but changing it is not allowed.

			    Preamble

  The licenses for most software are designed to take away your
freedom to share and change it.  By contrast, the GNU General Public
License is intended to guarantee your freedom to share and change free
software--to make sure the software is free for all its users.  This
General Public License applies to most of the Free Software
Foundation's software and to any other program whose authors commit to
using it.  (Some other Free Software Foundation software is covered by
the GNU Library General Public License instead.)  You can apply it to
your programs, too.

  When we speak of free software, we are referring to freedom, not
price.  Our General Public Licenses are designed to make sure that you
have the freedom to distribute copies of free software (and charge for
this service if you wish), that you receive source code or can get it
if you want it, that you can change the software or use pieces of it
in new free programs; and that you know you can do these things.

  To protect your rights, we need to make restrictions that forbid
anyone to deny you these rights or to ask you to surrender the rights.
These restrictions translate to certain responsibilities for you if you
distribute copies of the software, or if you modify it.

  For example, if you distribute copies of such a program, whether
gratis or for a fee, you must give the recipients all the rights that
you have.  You must make sure that they, too, receive or can get the
source code.  And you must show them these terms so they know their
rights.

  We protect your rights with two steps: (1) copyright the software, and
(2) offer you this license which gives you legal permission to copy,
distribute and/or modify the software.

  Also, for each author's protection and ours, we want to make certain
that everyone understands that there is no warranty for this free
software.  If the software is modified by someone else and passed on, we
want its recipients to know that what they have is not the original, so
that any problems introduced by others will not reflect on the original
authors' reputations.

  Finally, any free program is threatened constantly by software
patents.  We wish to avoid the danger that redistributors of a free
program will individually obtain patent licenses, in effect making the
program proprietary.  To prevent this, we have made it clear that any
patent must be licensed for everyone's free use or not licensed at all.

  The precise terms and conditions for copying, distribution and
modification follow.

		    GNU GENERAL PUBLIC LICENSE
   TERMS AND CONDITIONS FOR COPYING, DISTRIBUTION AND MODIFICATION

  0. This License applies to any program or other work which contains
a notice placed by the copyright holder saying it may be distributed
under the terms of this General Public License.  The "Program", below,
refers to any such program or work, and a "work based on the Program"
means either the Program or any derivative work under copyright law:
that is to say, a work containing the Program or a portion of it,
either verbatim or with modifications and/or translated into another
language.  (Hereinafter, translation is included without limitation in
the term "modification".)  Each licensee is addressed as "you".

Activities other than copying, distribution and modification are not
covered by this License; they are outside its scope.  The act of
running the Program is not restricted, and the output from the Program
is covered only if its contents constitute a work based on the
Program (independent of having been made by running the Program).
Whether that is true depends on what the Program does.

  1. You may copy and distribute verbatim copies of the Program's
source code as you receive it, in any medium, provided that you
conspicuously and appropriately publish on each copy an appropriate
copyright notice and disclaimer of warranty; keep intact all the
notices that refer to this License and to the absence of any warranty;
and give any other recipients of the Program a copy of this License
along with the Program.

You may charge a fee for the physical act of transferring a copy, and
you may at your option offer warranty protection in exchange for a fee.

  2. You may modify your copy or copies of the Program or any portion
of it, thus forming a work based on the Program, and copy and
distribute such modifications or work under the terms of Section 1
above, provided that you also meet all of these conditions:

    a) You must cause the modified files to carry prominent notices
    stating that you changed the files and the date of any change.

    b) You must cause any work that you distribute or publish, that in
    whole or in part contains or is derived from the Program or any
    part thereof, to be licensed as a whole at no charge to all third
    parties under the terms of this License.

    c) If the modified program normally reads commands interactively
    when run, you must cause it, when started running for such
    interactive use in the most ordinary way, to print or display an
    announcement including an appropriate copyright notice and a
    notice that there is no warranty (or else, saying that you provide
    a warranty) and that users may redistribute the program under
    these conditions, and telling the user how to view a copy of this
    License.  (Exception: if the Program itself is interactive but
    does not normally print such an announcement, your work based on
    the Program is not required to print an announcement.)

These requirements apply to the modified work as a whole.  If
identifiable sections of that work are not derived from the Program,
and can be reasonably considered independent and separate works in
themselves, then this License, and its terms, do not apply to those
sections when you distribute them as separate works.  But when you
distribute the same sections as part of a whole which is a work based
on the Program, the distribution of the whole must be on the terms of
this License, whose permissions for other licensees extend to the
entire whole, and thus to each and every part regardless of who wrote it.

Thus, it is not the intent of this section to claim rights or contest
your rights to work written entirely by you; rather, the intent is to
exercise the right to control the distribution of derivative or
collective works based on the Program.

In addition, mere aggregation of another work not based on the Program
with the Program (or with a work based on the Program) on a volume of
a storage or distribution medium does not bring the other work under
the scope of this License.

  3. You may copy and distribute the Program (or a work based on it,
under Section 2) in object code or executable form under the terms of
Sections 1 and 2 above provided that you also do one of the following:

    a) Accompany it with the complete corresponding machine-readable
    source code, which must be distributed under the terms of Sections
    1 and 2 above on a medium customarily used for software interchange; or,

    b) Accompany it with a written offer, valid for at least three
    years, to give any third party, for a charge no more than your
    cost of physically performing source distribution, a complete
    machine-readable copy of the corresponding source code, to be
    distributed under the terms of Sections 1 and 2 above on a medium
    customarily used for software interchange; or,

    c) Accompany it with the information you received as to the offer
    to distribute corresponding source code.  (This alternative is
    allowed only for noncommercial distribution and only if you
    received the program in object code or executable form with such
    an offer, in accord with Subsection b above.)

The source code for a work means the preferred form of the work for
making modifications to it.  For an executable work, complete source
code means all the source code for all modules it contains, plus any
associated interface definition files, plus the scripts used to
control compilation and installation of the executable.  However, as a
special exception, the source code distributed need not include
anything that is normally distributed (in either source or binary
form) with the major components (compiler, kernel, and so on) of the
operating system on which the executable runs, unless that component
itself accompanies the executable.

If distribution of executable or object code is made by offering
access to copy from a designated place, then offering equivalent
access to copy the source code from the same place counts as
distribution of the source code, even though third parties are not
compelled to copy the source along with the object code.

  4. You may not copy, modify, sublicense, or distribute the Program
except as expressly provided under this License.  Any attempt
otherwise to copy, modify, sublicense or distribute the Program is
void, and will automatically terminate your rights under this License.
However, parties who have received copies, or rights, from you under
this License will not have their licenses terminated so long as such
parties remain in full compliance.

  5. You are not required to accept this License, since you have not
signed it.  However, nothing else grants you permission to modify or
distribute the Program or its derivative works.  These actions are
prohibited by law if you do not accept this License.  Therefore, by
modifying or distributing the Program (or any work based on the
Program), you indicate your acceptance of this License to do so, and
all its terms and conditions for copying, distributing or modifying
the Program or works based on it.

  6. Each time you redistribute the Program (or any work based on the
Program), the recipient automatically receives a license from the
original licensor to copy, distribute or modify the Program subject to
these terms and conditions.  You may not impose any further
restrictions on the recipients' exercise of the rights granted herein.
You are not responsible for enforcing compliance by third parties to
this License.

  7. If, as a consequence of a court judgment or allegation of patent
infringement or for any other reason (not limited to patent issues),
conditions are imposed on you (whether by court order, agreement or
otherwise) that contradict the conditions of this License, they do not
excuse you from the conditions of this License.  If you cannot
distribute so as to satisfy simultaneously your obligations under this
License and any other pertinent obligations, then as a consequence you
may not distribute the Program at all.  For example, if a patent
license would not permit royalty-free redistribution of the Program by
all those who receive copies directly or indirectly through you, then
the only way you could satisfy both it and this License would be to
refrain entirely from distribution of the Program.

If any portion of this section is held invalid or unenforceable under
any particular circumstance, the balance of the section is intended to
apply and the section as a whole is intended to apply in other
circumstances.

It is not the purpose of this section to induce you to infringe any
patents or other property right claims or to contest validity of any
such claims; this section has the sole purpose of protecting the
integrity of the free software distribution system, which is
implemented by public license practices.  Many people have made
generous contributions to the wide range of software distributed
through that system in reliance on consistent application of that
system; it is up to the author/donor to decide if he or she is willing
to distribute software through any other system and a licensee cannot
impose that choice.

This section is intended to make thoroughly clear what is believed to
be a consequence of the rest of this License.

  8. If the distribution and/or use of the Program is restricted in
certain countries either by patents or by copyrighted interfaces, the
original copyright holder who places the Program under this License
may add an explicit geographical distribution limitation excluding
those countries, so that distribution is permitted only in or among
countries not thus excluded.  In such case, this License incorporates
the limitation as if written in the body of this License.

  9. The Free Software Foundation may publish revised and/or new versions
of the General Public License from time to time.  Such new versions will
be similar in spirit to the present version, but may differ in detail to
address new problems or concerns.

Each version is given a distinguishing version number.  If the Program
specifies a version number of this License which applies to it and "any
later version", you have the option of following the terms and conditions
either of that version or of any later version published by the Free
Software Foundation.  If the Program does not specify a version number of
this License, you may choose any version ever published by the Free Software
Foundation.

  10. If you wish to incorporate parts of the Program into other free
programs whose distribution conditions are different, write to the author
to ask for permission.  For software which is copyrighted by the Free
Software Foundation, write to the Free Software Foundation; we sometimes
make exceptions for this.  Our decision will be guided by the two goals
of preserving the free status of all derivatives of our free software and
of promoting the sharing and reuse of software generally.

			    NO WARRANTY

  11. BECAUSE THE PROGRAM IS LICENSED FREE OF CHARGE, THERE IS NO WARRANTY
FOR THE PROGRAM, TO THE EXTENT PERMITTED BY APPLICABLE LAW.  EXCEPT WHEN
OTHERWISE STATED IN WRITING THE COPYRIGHT HOLDERS AND/OR OTHER PARTIES
PROVIDE THE PROGRAM "AS IS" WITHOUT WARRANTY OF ANY KIND, EITHER EXPRESSED
OR IMPLIED, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE.  THE ENTIRE RISK AS
TO THE QUALITY AND PERFORMANCE OF THE PROGRAM IS WITH YOU.  SHOULD THE
PROGRAM PROVE DEFECTIVE, YOU ASSUME THE COST OF ALL NECESSARY SERVICING,
REPAIR OR CORRECTION.

  12. IN NO EVENT UNLESS REQUIRED BY APPLICABLE LAW OR AGREED TO IN WRITING
WILL ANY COPYRIGHT HOLDER, OR ANY OTHER PARTY WHO MAY MODIFY AND/OR
REDISTRIBUTE THE PROGRAM AS PERMITTED ABOVE, BE LIABLE TO YOU FOR DAMAGES,
INCLUDING ANY GENERAL, SPECIAL, INCIDENTAL OR CONSEQUENTIAL DAMAGES ARISING
OUT OF THE USE OR INABILITY TO USE THE PROGRAM (INCLUDING BUT NOT LIMITED
TO LOSS OF DATA OR DATA BEING RENDERED INACCURATE OR LOSSES SUSTAINED BY
YOU OR THIRD PARTIES OR A FAILURE OF THE PROGRAM TO OPERATE WITH ANY OTHER
PROGRAMS), EVEN IF SUCH HOLDER OR OTHER PARTY HAS BEEN ADVISED OF THE
POSSIBILITY OF SUCH DAMAGES.

		     END OF TERMS AND CONDITIONS

	    How to Apply These Terms to Your New Programs

  If you develop a new program, and you want it to be of the greatest
possible use to the public, the best way to achieve this is to make it
free software which everyone can redistribute and change under these terms.

  To do so, attach the following notices to the program.  It is safest
to attach them to the start of each source file to most effectively
convey the exclusion of warranty; and each file should have at least
the "copyright" line and a pointer to where the full notice is found.

    <one line to give the program's name and a brief idea of what it does.>
    Copyright (C) <year>  <name of author>

    This program is free software; you can redistribute it and/or modify
    it under the terms of the GNU General Public License as published by
    the Free Software Foundation; either version 2 of the License, or
    (at your option) any later version.

    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
    GNU General Public License for more details.

    You should have received a copy of the GNU General Public License
    along with this program; if not, write to the Free Software
    Foundation, Inc., 59 Temple Place, Suite 330, Boston, MA  02111-1307  USA


Also add information on how to contact you by electronic and paper mail.

If the program is interactive, make it output a short notice like this
when it starts in an interactive mode:

    Gnomovision version 69, Copyright (C) year name of author
    Gnomovision comes with ABSOLUTELY NO WARRANTY; for details type `show w'.
    This is free software, and you are welcome to redistribute it
    under certain conditions; type `show c' for details.

The hypothetical commands `show w' and `show c' should show the appropriate
parts of the General Public License.  Of course, the commands you use may
be called something other than `show w' and `show c'; they could even be
mouse-clicks or menu items--whatever suits your program.

You should also get your employer (if you work as a programmer) or your
school, if any, to sign a "copyright disclaimer" for the program, if
necessary.  Here is a sample; alter the names:

  Yoyodyne, Inc., hereby disclaims all copyright interest in the program
  `Gnomovision' (which makes passes at compilers) written by James Hacker.

  <signature of Ty Coon>, 1 April 1989
  Ty Coon, President of Vice

This General Public License does not permit incorporating your program into
proprietary programs.  If your program is a subroutine library, you may
consider it more useful to permit linking proprietary applications with the
library.  If this is what you want to do, use the GNU Library General
Public License instead of this License.

----------------------------------------------------------------------

The bundled ZLib code is licensed under the ZLib license:

 (C) 1995-2022 Jean-loup Gailly and Mark Adler

  This software is provided 'as-is', without any express or implied
  warranty.  In no event will the authors be held liable for any damages
  arising from the use of this software.

  Permission is granted to anyone to use this software for any purpose,
  including commercial applications, and to alter it and redistribute it
  freely, subject to the following restrictions:

  1. The origin of this software must not be misrepresented; you must not
     claim that you wrote the original software. If you use this software
     in a product, an acknowledgment in the product documentation would be
     appreciated but is not required.
  2. Altered source versions must be plainly marked as such, and must not be
     misrepresented as being the original software.
  3. This notice may not be removed or altered from any source distribution.

  Jean-loup Gailly        Mark Adler
  jloup@gzip.org          madler@alumni.caltech.edu

----------------------------------------------------------------------

The Clar framework is licensed under the ISC license:

Copyright (c) 2011-2015 Vicent Marti

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

----------------------------------------------------------------------

The bundled PCRE implementation (deps/pcre/) is licensed under the BSD
license.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

    * Redistributions of source code must retain the above copyright notice,
      this list of conditions and the following disclaimer.

    * Redistributions in binary form must reproduce the above copyright
      notice, this list of conditions and the following disclaimer in the
      documentation and/or other materials provided with the distribution.

    * Neither the name of the University of Cambridge nor the name of Google
      Inc. nor the names of their contributors may be used to endorse or
      promote products derived from this software without specific prior
      written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE
LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.

----------------------------------------------------------------------

The bundled winhttp definition files (deps/winhttp/) are licensed under
the GNU LGPL (available at the end of this file).

Copyright (C) 2007 Francois Gouget

This library is free software; you can redistribute it and/or
modify it under the terms of the GNU Lesser General Public
License as published by the Free Software Foundation; either
version 2.1 of the License, or (at your option) any later version.

This library is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
Lesser General Public License for more details.

You should have received a copy of the GNU Lesser General Public
License along with this library; if not, write to the Free Software
Foundation, Inc., 51 Franklin St, Fifth Floor, Boston, MA 02110-1301, USA

----------------------------------------------------------------------

                  GNU LESSER GENERAL PUBLIC LICENSE
                       Version 2.1, February 1999

 Copyright (C) 1991, 1999 Free Software Foundation, Inc.
 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 Everyone is permitted to copy and distribute verbatim copies
 of this license document, but changing it is not allowed.

[This is the first released version of the Lesser GPL.  It also counts
 as the successor of the GNU Library Public License, version 2, hence
 the version number 2.1.]

                            Preamble

  The licenses for most software are designed to take away your
freedom to share and change it.  By contrast, the GNU General Public
Licenses are intended to guarantee your freedom to share and change
free software--to make sure the software is free for all its users.

  This license, the Lesser General Public License, applies to some
specially designated software packages--typically libraries--of the
Free Software Foundation and other authors who decide to use it.  You
can use it too, but we suggest you first think carefully about whether
this license or the ordinary General Public License is the better
strategy to use in any particular case, based on the explanations below.

  When we speak of free software, we are referring to freedom of use,
not price.  Our General Public Licenses are designed to make sure that
you have the freedom to distribute copies of free software (and charge
for this service if you wish); that you receive source code or can get
it if you want it; that you can change the software and use pieces of
it in new free programs; and that you are informed that you can do
these things.

  To protect your rights, we need to make restrictions that forbid
distributors to deny you these rights or to ask you to surrender these
rights.  These restrictions translate to certain responsibilities for
you if you distribute copies of the library or if you modify it.

  For example, if you distribute copies of the library, whether gratis
or for a fee, you must give the recipients all the rights that we gave
you.  You must make sure that they, too, receive or can get the source
code.  If you link other code with the library, you must provide
complete object files to the recipients, so that they can relink them
with the library after making changes to the library and recompiling
it.  And you must show them these terms so they know their rights.

  We protect your rights with a two-step method: (1) we copyright the
library, and (2) we offer you this license, which gives you legal
permission to copy, distribute and/or modify the library.

  To protect each distributor, we want to make it very clear that
there is no warranty for the free library.  Also, if the library is
modified by someone else and passed on, the recipients should know
that what they have is not the original version, so that the original
author's reputation will not be affected by problems that might be
introduced by others.

  Finally, software patents pose a constant threat to the existence of
any free program.  We wish to make sure that a company cannot
effectively restrict the users of a free program by obtaining a
restrictive license from a patent holder.  Therefore, we insist that
any patent license obtained for a version of the library must be
consistent with the full freedom of use specified in this license.

  Most GNU software, including some libraries, is covered by the
ordinary GNU General Public License.  This license, the GNU Lesser
General Public License, applies to certain designated libraries, and
is quite different from the ordinary General Public License.  We use
this license for certain libraries in order to permit linking those
libraries into non-free programs.

  When a program is linked with a library, whether statically or using
a shared library, the combination of the two is legally speaking a
combined work, a derivative of the original library.  The ordinary
General Public License therefore permits such linking only if the
entire combination fits its criteria of freedom.  The Lesser General
Public License permits more lax criteria for linking other code with
the library.

  We call this license the "Lesser" General Public License because it
does Less to protect the user's freedom than the ordinary General
Public License.  It also provides other free software developers Less
of an advantage over competing non-free programs.  These disadvantages
are the reason we use the ordinary General Public License for many
libraries.  However, the Lesser license provides advantages in certain
special circumstances.

  For example, on rare occasions, there may be a special need to
encourage the widest possible use of a certain library, so that it becomes
a de-facto standard.  To achieve this, non-free programs must be
allowed to use the library.  A more frequent case is that a free
library does the same job as widely used non-free libraries.  In this
case, there is little to gain by limiting the free library to free
software only, so we use the Lesser General Public License.

  In other cases, permission to use a particular library in non-free
programs enables a greater number of people to use a large body of
free software.  For example, permission to use the GNU C Library in
non-free programs enables many more people to use the whole GNU
operating system, as well as its variant, the GNU/Linux operating
system.

  Although the Lesser General Public License is Less protective of the
users' freedom, it does ensure that the user of a program that is
linked with the Library has the freedom and the wherewithal to run
that program using a modified version of the Library.

  The precise terms and conditions for copying, distribution and
modification follow.  Pay close attention to the difference between a
"work based on the library" and a "work that uses the library".  The
former contains code derived from the library, whereas the latter must
be combined with the library in order to run.

                  GNU LESSER GENERAL PUBLIC LICENSE
   TERMS AND CONDITIONS FOR COPYING, DISTRIBUTION AND MODIFICATION

  0. This License Agreement applies to any software library or other
program which contains a notice placed by the copyright holder or
other authorized party saying it may be distributed under the terms of
this Lesser General Public License (also called "this License").
Each licensee is addressed as "you".

  A "library" means a collection of software functions and/or data
prepared so as to be conveniently linked with application programs
(which use some of those functions and data) to form executables.

  The "Library", below, refers to any such software library or work
which has been distributed under these terms.  A "work based on the
Library" means either the Library or any derivative work under
copyright law: that is to say, a work containing the Library or a
portion of it, either verbatim or with modifications and/or translated
straightforwardly into another language.  (Hereinafter, translation is
included without limitation in the term "modification".)

  "Source code" for a work means the preferred form of the work for
making modifications to it.  For a library, complete source code means
all the source code for all modules it contains, plus any associated
interface definition files, plus the scripts used to control compilation
and installation of the library.

  Activities other than copying, distribution and modification are not
covered by this License; they are outside its scope.  The act of
running a program using the Library is not restricted, and output from
such a program is covered only if its contents constitute a work based
on the Library (independent of the use of the Library in a tool for
writing it).  Whether that is true depends on what the Library does
and what the program that uses the Library does.

  1. You may copy and distribute verbatim copies of the Library's
complete source code as you receive it, in any medium, provided that
you conspicuously and appropriately publish on each copy an
appropriate copyright notice and disclaimer of warranty; keep intact
all the notices that refer to this License and to the absence of any
warranty; and distribute a copy of this License along with the
Library.

  You may charge a fee for the physical act of transferring a copy,
and you may at your option offer warranty protection in exchange for a
fee.

  2. You may modify your copy or copies of the Library or any portion
of it, thus forming a work based on the Library, and copy and
distribute such modifications or work under the terms of Section 1
above, provided that you also meet all of these conditions:

    a) The modified work must itself be a software library.

    b) You must cause the files modified to carry prominent notices
    stating that you changed the files and the date of any change.

    c) You must cause the whole of the work to be licensed at no
    charge to all third parties under the terms of this License.

    d) If a facility in the modified Library refers to a function or a
    table of data to be supplied by an application program that uses
    the facility, other than as an argument passed when the facility
    is invoked, then you must make a good faith effort to ensure that,
    in the event an application does not supply such function or
    table, the facility still operates, and performs whatever part of
    its purpose remains meaningful.

    (For example, a function in a library to compute square roots has
    a purpose that is entirely well-defined independent of the
    application.  Therefore, Subsection 2d requires that any
    application-supplied function or table used by this function must
    be optional: if the application does not supply it, the square
    root function must still compute square roots.)

These requirements apply to the modified work as a whole.  If
identifiable sections of that work are not derived from the Library,
and can be reasonably considered independent and separate works in
themselves, then this License, and its terms, do not apply to those
sections when you distribute them as separate works.  But when you
distribute the same sections as part of a whole which is a work based
on the Library, the distribution of the whole must be on the terms of
this License, whose permissions for other licensees extend to the
entire whole, and thus to each and every part regardless of who wrote
it.

Thus, it is not the intent of this section to claim rights or contest
your rights to work written entirely by you; rather, the intent is to
exercise the right to control the distribution of derivative or
collective works based on the Library.

In addition, mere aggregation of another work not based on the Library
with the Library (or with a work based on the Library) on a volume of
a storage or distribution medium does not bring the other work under
the scope of this License.

  3. You may opt to apply the terms of the ordinary GNU General Public
License instead of this License to a given copy of the Library.  To do
this, you must alter all the notices that refer to this License, so
that they refer to the ordinary GNU General Public License, version 2,
instead of to this License.  (If a newer version than version 2 of the
ordinary GNU General Public License has appeared, then you can specify
that version instead if you wish.)  Do not make any other change in
these notices.

  Once this change is made in a given copy, it is irreversible for
that copy, so the ordinary GNU General Public License applies to all
subsequent copies and derivative works made from that copy.

  This option is useful when you wish to copy part of the code of
the Library into a program that is not a library.

  4. You may copy and distribute the Library (or a portion or
derivative of it, under Section 2) in object code or executable form
under the terms of Sections 1 and 2 above provided that you accompany
it with the complete corresponding machine-readable source code, which
must be distributed under the terms of Sections 1 and 2 above on a
medium customarily used for software interchange.

  If distribution of object code is made by offering access to copy
from a designated place, then offering equivalent access to copy the
source code from the same place satisfies the requirement to
distribute the source code, even though third parties are not
compelled to copy the source along with the object code.

  5. A program that contains no derivative of any portion of the
Library, but is designed to work with the Library by being compiled or
linked with it, is called a "work that uses the Library".  Such a
work, in isolation, is not a derivative work of the Library, and
therefore falls outside the scope of this License.

  However, linking a "work that uses the Library" with the Library
creates an executable that is a derivative of the Library (because it
contains portions of the Library), rather than a "work that uses the
library".  The executable is therefore covered by this License.
Section 6 states terms for distribution of such executables.

  When a "work that uses the Library" uses material from a header file
that is part of the Library, the object code for the work may be a
derivative work of the Library even though the source code is not.
Whether this is true is especially significant if the work can be
linked without the Library, or if the work is itself a library.  The
threshold for this to be true is not precisely defined by law.

  If such an object file uses only numerical parameters, data
structure layouts and accessors, and small macros and small inline
functions (ten lines or less in length), then the use of the object
file is unrestricted, regardless of whether it is legally a derivative
work.  (Executables containing this object code plus portions of the
Library will still fall under Section 6.)

  Otherwise, if the work is a derivative of the Library, you may
distribute the object code for the work under the terms of Section 6.
Any executables containing that work also fall under Section 6,
whether or not they are linked directly with the Library itself.

  6. As an exception to the Sections above, you may also combine or
link a "work that uses the Library" with the Library to produce a
work containing portions of the Library, and distribute that work
under terms of your choice, provided that the terms permit
modification of the work for the customer's own use and reverse
engineering for debugging such modifications.

  You must give prominent notice with each copy of the work that the
Library is used in it and that the Library and its use are covered by
this License.  You must supply a copy of this License.  If the work
during execution displays copyright notices, you must include the
copyright notice for the Library among them, as well as a reference
directing the user to the copy of this License.  Also, you must do one
of these things:

    a) Accompany the work with the complete corresponding
    machine-readable source code for the Library including whatever
    changes were used in the work (which must be distributed under
    Sections 1 and 2 above); and, if the work is an executable linked
    with the Library, with the complete machine-readable "work that
    uses the Library", as object code and/or source code, so that the
    user can modify the Library and then relink to produce a modified
    executable containing the modified Library.  (It is understood
    that the user who changes the contents of definitions files in the
    Library will not necessarily be able to recompile the application
    to use the modified definitions.)

    b) Use a suitable shared library mechanism for linking with the
    Library.  A suitable mechanism is one that (1) uses at run time a
    copy of the library already present on the user's computer system,
    rather than copying library functions into the executable, and (2)
    will operate properly with a modified version of the library, if
    the user installs one, as long as the modified version is
    interface-compatible with the version that the work was made with.

    c) Accompany the work with a written offer, valid for at
    least three years, to give the same user the materials
    specified in Subsection 6a, above, for a charge no more
    than the cost of performing this distribution.

    d) If distribution of the work is made by offering access to copy
    from a designated place, offer equivalent access to copy the above
    specified materials from the same place.

    e) Verify that the user has already received a copy of these
    materials or that you have already sent this user a copy.

  For an executable, the required form of the "work that uses the
Library" must include any data and utility programs needed for
reproducing the executable from it.  However, as a special exception,
the materials to be distributed need not include anything that is
normally distributed (in either source or binary form) with the major
components (compiler, kernel, and so on) of the operating system on
which the executable runs, unless that component itself accompanies
the executable.

  It may happen that this requirement contradicts the license
restrictions of other proprietary libraries that do not normally
accompany the operating system.  Such a contradiction means you cannot
use both them and the Library together in an executable that you
distribute.

  7. You may place library facilities that are a work based on the
Library side-by-side in a single library together with other library
facilities not covered by this License, and distribute such a combined
library, provided that the separate distribution of the work based on
the Library and of the other library facilities is otherwise
permitted, and provided that you do these two things:

    a) Accompany the combined library with a copy of the same work
    based on the Library, uncombined with any other library
    facilities.  This must be distributed under the terms of the
    Sections above.

    b) Give prominent notice with the combined library of the fact
    that part of it is a work based on the Library, and explaining
    where to find the accompanying uncombined form of the same work.

  8. You may not copy, modify, sublicense, link with, or distribute
the Library except as expressly provided under this License.  Any
attempt otherwise to copy, modify, sublicense, link with, or
distribute the Library is void, and will automatically terminate your
rights under this License.  However, parties who have received copies,
or rights, from you under this License will not have their licenses
terminated so long as such parties remain in full compliance.

  9. You are not required to accept this License, since you have not
signed it.  However, nothing else grants you permission to modify or
distribute the Library or its derivative works.  These actions are
prohibited by law if you do not accept this License.  Therefore, by
modifying or distributing the Library (or any work based on the
Library), you indicate your acceptance of this License to do so, and
all its terms and conditions for copying, distributing or modifying
the Library or works based on it.

  10. Each time you redistribute the Library (or any work based on the
Library), the recipient automatically receives a license from the
original licensor to copy, distribute, link with or modify the Library
subject to these terms and conditions.  You may not impose any further
restrictions on the recipients' exercise of the rights granted herein.
You are not responsible for enforcing compliance by third parties with
this License.

  11. If, as a consequence of a court judgment or allegation of patent
infringement or for any other reason (not limited to patent issues),
conditions are imposed on you (whether by court order, agreement or
otherwise) that contradict the conditions of this License, they do not
excuse you from the conditions of this License.  If you cannot
distribute so as to satisfy simultaneously your obligations under this
License and any other pertinent obligations, then as a consequence you
may not distribute the Library at all.  For example, if a patent
license would not permit royalty-free redistribution of the Library by
all those who receive copies directly or indirectly through you, then
the only way you could satisfy both it and this License would be to
refrain entirely from distribution of the Library.

If any portion of this section is held invalid or unenforceable under any
particular circumstance, the balance of the section is intended to apply,
and the section as a whole is intended to apply in other circumstances.

It is not the purpose of this section to induce you to infringe any
patents or other property right claims or to contest validity of any
such claims; this section has the sole purpose of protecting the
integrity of the free software distribution system which is
implemented by public license practices.  Many people have made
generous contributions to the wide range of software distributed
through that system in reliance on consistent application of that
system; it is up to the author/donor to decide if he or she is willing
to distribute software through any other system and a licensee cannot
impose that choice.

This section is intended to make thoroughly clear what is believed to
be a consequence of the rest of this License.

  12. If the distribution and/or use of the Library is restricted in
certain countries either by patents or by copyrighted interfaces, the
original copyright holder who places the Library under this License may add
an explicit geographical distribution limitation excluding those countries,
so that distribution is permitted only in or among countries not thus
excluded.  In such case, this License incorporates the limitation as if
written in the body of this License.

  13. The Free Software Foundation may publish revised and/or new
versions of the Lesser General Public License from time to time.
Such new versions will be similar in spirit to the present version,
but may differ in detail to address new problems or concerns.

Each version is given a distinguishing version number.  If the Library
specifies a version number of this License which applies to it and
"any later version", you have the option of following the terms and
conditions either of that version or of any later version published by
the Free Software Foundation.  If the Library does not specify a
license version number, you may choose any version ever published by
the Free Software Foundation.

  14. If you wish to incorporate parts of the Library into other free
programs whose distribution conditions are incompatible with these,
write to the author to ask for permission.  For software which is
copyrighted by the Free Software Foundation, write to the Free
Software Foundation; we sometimes make exceptions for this.  Our
decision will be guided by the two goals of preserving the free status
of all derivatives of our free software and of promoting the sharing
and reuse of software generally.

                            NO WARRANTY

  15. BECAUSE THE LIBRARY IS LICENSED FREE OF CHARGE, THERE IS NO
WARRANTY FOR THE LIBRARY, TO THE EXTENT PERMITTED BY APPLICABLE LAW.
EXCEPT WHEN OTHERWISE STATED IN WRITING THE COPYRIGHT HOLDERS AND/OR
OTHER PARTIES PROVIDE THE LIBRARY "AS IS" WITHOUT WARRANTY OF ANY
KIND, EITHER EXPRESSED OR IMPLIED, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR
PURPOSE.  THE ENTIRE RISK AS TO THE QUALITY AND PERFORMANCE OF THE
LIBRARY IS WITH YOU.  SHOULD THE LIBRARY PROVE DEFECTIVE, YOU ASSUME
THE COST OF ALL NECESSARY SERVICING, REPAIR OR CORRECTION.

  16. IN NO EVENT UNLESS REQUIRED BY APPLICABLE LAW OR AGREED TO IN
WRITING WILL ANY COPYRIGHT HOLDER, OR ANY OTHER PARTY WHO MAY MODIFY
AND/OR REDISTRIBUTE THE LIBRARY AS PERMITTED ABOVE, BE LIABLE TO YOU
FOR DAMAGES, INCLUDING ANY GENERAL, SPECIAL, INCIDENTAL OR
CONSEQUENTIAL DAMAGES ARISING OUT OF THE USE OR INABILITY TO USE THE
LIBRARY (INCLUDING BUT NOT LIMITED TO LOSS OF DATA OR DATA BEING
RENDERED INACCURATE OR LOSSES SUSTAINED BY YOU OR THIRD PARTIES OR A
FAILURE OF THE LIBRARY TO OPERATE WITH ANY OTHER SOFTWARE), EVEN IF
SUCH HOLDER OR OTHER PARTY HAS BEEN ADVISED OF THE POSSIBILITY OF SUCH
DAMAGES.

                     END OF TERMS AND CONDITIONS

           How to Apply These Terms to Your New Libraries

  If you develop a new library, and you want it to be of the greatest
possible use to the public, we recommend making it free software that
everyone can redistribute and change.  You can do so by permitting
redistribution under these terms (or, alternatively, under the terms of the
ordinary General Public License).

  To apply these terms, attach the following notices to the library.  It is
safest to attach them to the start of each source file to most effectively
convey the exclusion of warranty; and each file should have at least the
"copyright" line and a pointer to where the full notice is found.

    <one line to give the library's name and a brief idea of what it does.>
    Copyright (C) <year>  <name of author>

    This library is free software; you can redistribute it and/or
    modify it under the terms of the GNU Lesser General Public
    License as published by the Free Software Foundation; either
    version 2.1 of the License, or (at your option) any later version.

    This library is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
    Lesser General Public License for more details.

    You should have received a copy of the GNU Lesser General Public
    License along with this library; if not, write to the Free Software
    Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA

Also add information on how to contact you by electronic and paper mail.

You should also get your employer (if you work as a programmer) or your
school, if any, to sign a "copyright disclaimer" for the library, if
necessary.  Here is a sample; alter the names:

  Yoyodyne, Inc., hereby disclaims all copyright interest in the
  library `Frob' (a library for tweaking knobs) written by James Random Hacker.

  <signature of Ty Coon>, 1 April 1990
  Ty Coon, President of Vice

That's all there is to it!

----------------------------------------------------------------------

The bundled SHA1 collision detection code is licensed under the MIT license:

MIT License

Copyright (c) 2017:
    Marc Stevens
    Cryptology Group
    Centrum Wiskunde & Informatica
    P.O. Box 94079, 1090 GB Amsterdam, Netherlands
    marc@marc-stevens.nl

    Dan Shumow
    Microsoft Research
    danshu@microsoft.com

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

----------------------------------------------------------------------

The bundled wildmatch code is licensed under the BSD license:

Copyright Rich Salz.
All rights reserved.

Redistribution and use in any form are permitted provided that the
following restrictions are are met:

1.  Source distributions must retain this entire copyright notice
    and comment.
2.  Binary distributions must include the acknowledgement ``This
    product includes software developed by Rich Salz'' in the
    documentation or other materials provided with the
    distribution.  This must not be represented as an endorsement
   or promotion without specific prior written permission.
3.  The origin of this software must not be misrepresented, either
    by explicit claim or by omission.  Credits must appear in the
    source and documentation.
4.  Altered versions must be plainly marked as such in the source
    and documentation and must not be misrepresented as being the
    original software.

THIS SOFTWARE IS PROVIDED ``AS IS'' AND WITHOUT ANY EXPRESS OR IMPLIED
WARRANTIES, INCLUDING, WITHOUT LIMITATION, THE IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE.

----------------------------------------------------------------------

Portions of the OpenSSL headers are included under the OpenSSL license:

Copyright (C) 1995-1998 Eric Young (eay@cryptsoft.com)
All rights reserved.

This package is an SSL implementation written
by Eric Young (eay@cryptsoft.com).
The implementation was written so as to conform with Netscapes SSL.

This library is free for commercial and non-commercial use as long as
the following conditions are aheared to.  The following conditions
apply to all code found in this distribution, be it the RC4, RSA,
lhash, DES, etc., code; not just the SSL code.  The SSL documentation
included with this distribution is covered by the same copyright terms
except that the holder is Tim Hudson (tjh@cryptsoft.com).

Copyright remains Eric Young's, and as such any Copyright notices in
the code are not to be removed.
If this package is used in a product, Eric Young should be given attribution
as the author of the parts of the library used.
This can be in the form of a textual message at program startup or
in documentation (online or textual) provided with the package.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:
1. Redistributions of source code must retain the copyright
   notice, this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.
3. All advertising materials mentioning features or use of this software
   must display the following acknowledgement:
   "This product includes cryptographic software written by
    Eric Young (eay@cryptsoft.com)"
   The word 'cryptographic' can be left out if the rouines from the library
   being used are not cryptographic related :-).
4. If you include any Windows specific code (or a derivative thereof) from
   the apps directory (application code) you must include an acknowledgement:
   "This product includes software written by Tim Hudson (tjh@cryptsoft.com)"

THIS SOFTWARE IS PROVIDED BY ERIC YOUNG ``AS IS'' AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
SUCH DAMAGE.

The licence and distribution terms for any publically available version or
derivative of this code cannot be changed.  i.e. this code cannot simply be
copied and put under another distribution licence
[including the GNU Public Licence.]

====================================================================
Copyright (c) 1998-2007 The OpenSSL Project.  All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:

1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in
   the documentation and/or other materials provided with the
   distribution.

3. All advertising materials mentioning features or use of this
   software must display the following acknowledgment:
   "This product includes software developed by the OpenSSL Project
   for use in the OpenSSL Toolkit. (http://www.openssl.org/)"

4. The names "OpenSSL Toolkit" and "OpenSSL Project" must not be used to
   endorse or promote products derived from this software without
   prior written permission. For written permission, please contact
   openssl-core@openssl.org.

5. Products derived from this software may not be called "OpenSSL"
   nor may "OpenSSL" appear in their names without prior written
   permission of the OpenSSL Project.

6. Redistributions of any form whatsoever must retain the following
   acknowledgment:
   "This product includes software developed by the OpenSSL Project
   for use in the OpenSSL Toolkit (http://www.openssl.org/)"

THIS SOFTWARE IS PROVIDED BY THE OpenSSL PROJECT ``AS IS'' AND ANY
EXPRESSED OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR
PURPOSE ARE DISCLAIMED.  IN NO EVENT SHALL THE OpenSSL PROJECT OR
ITS CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT
NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT,
STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED
OF THE POSSIBILITY OF SUCH DAMAGE.

----------------------------------------------------------------------

The xoroshiro256** implementation is licensed in the public domain:

Written in 2018 by David Blackman and Sebastiano Vigna (vigna@acm.org)

To the extent possible under law, the author has dedicated all copyright
and related and neighboring rights to this software to the public domain
worldwide. This software is distributed without any warranty.

See <http://creativecommons.org/publicdomain/zero/1.0/>.

----------------------------------------------------------------------

The built-in SHA256 support (src/hash/rfc6234) is taken from RFC 6234
under the following license:

Copyright (c) 2011 IETF Trust and the persons identified as
authors of the code.  All rights reserved.

Redistribution and use in source and binary forms, with or
without modification, are permitted provided that the following
conditions are met:

- Redistributions of source code must retain the above
  copyright notice, this list of conditions and
  the following disclaimer.

- Redistributions in binary form must reproduce the above
  copyright notice, this list of conditions and the following
  disclaimer in the documentation and/or other materials provided
  with the distribution.

- Neither the name of Internet Society, IETF or IETF Trust, nor
  the names of specific contributors, may be used to endorse or
  promote products derived from this software without specific
  prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND
CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED.  IN NO EVENT SHALL THE COPYRIGHT OWNER OR
CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT
NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR
OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE,
EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

----------------------------------------------------------------------

The built-in git_fs_path_basename_r() function is based on the
Android implementation, BSD licensed:

Copyright (C) 2008 The Android Open Source Project
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:
* Redistributions of source code must retain the above copyright
  notice, this list of conditions and the following disclaimer.
* Redistributions in binary form must reproduce the above copyright
  notice, this list of conditions and the following disclaimer in
  the documentation and/or other materials provided with the
  distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
AS IS AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS
FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE
COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT,
INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING,
BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS
OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED
AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT
OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
SUCH DAMAGE.

----------------------------------------------------------------------

The bundled ntlmclient code is licensed under the MIT license:

Copyright (c) Edward Thomson.  All rights reserved.

Permission is hereby granted, free of charge, to any person obtaining a
copy of this software and associated documentation files (the "Software"),
to deal in the Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included
in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL
THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.

----------------------------------------------------------------------

Portions of this software derived from Team Explorer Everywhere:

Copyright (c) Microsoft Corporation

All rights reserved.

Permission is hereby granted, free of charge, to any person obtaining a
copy of this software and associated documentation files (the "Software"),
to deal in the Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included
in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED *AS IS*, WITHOUT WARRANTY OF ANY KIND, EXPRESS
OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL
THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR
OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
OTHER DEALINGS IN THE SOFTWARE.

---------------------------------------------------------------------------

Portions of this software derived from the LLVM Compiler Infrastructure:

Copyright (c) 2003-2016 University of Illinois at Urbana-Champaign.
All rights reserved.

Developed by:

    LLVM Team

    University of Illinois at Urbana-Champaign

    http://llvm.org

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal with
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

    * Redistributions of source code must retain the above copyright notice,
      this list of conditions and the following disclaimers.

    * Redistributions in binary form must reproduce the above copyright notice,
      this list of conditions and the following disclaimers in the
      documentation and/or other materials provided with the distribution.

    * Neither the names of the LLVM Team, University of Illinois at
      Urbana-Champaign, nor the names of its contributors may be used to
      endorse or promote products derived from this Software without specific
      prior written permission.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL THE
CONTRIBUTORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS WITH THE
SOFTWARE.

---------------------------------------------------------------------------

Portions of this software derived from Unicode, Inc:

Copyright 2001-2004 Unicode, Inc.

Disclaimer

This source code is provided as is by Unicode, Inc. No claims are
made as to fitness for any particular purpose. No warranties of any
kind are expressed or implied. The recipient agrees to determine
applicability of information provided. If this file has been
purchased on magnetic or optical media from Unicode, Inc., the
sole remedy for any claim will be exchange of defective media
within 90 days of receipt.

Limitations on Rights to Redistribute This Code

Unicode, Inc. hereby grants the right to freely use the information
supplied in this file in the creation of products supporting the
Unicode Standard, and to make copies of this file in any form
for internal or external distribution as long as this notice
remains attached.

---------------------------------------------------------------------------

Portions of this software derived from sheredom/utf8.h:

This is free and unencumbered software released into the public domain.

Anyone is free to copy, modify, publish, use, compile, sell, or
distribute this software, either in source code form or as a compiled
binary, for any purpose, commercial or non-commercial, and by any
means.

In jurisdictions that recognize copyright laws, the author or authors
of this software dedicate any and all copyright interest in the
software to the public domain. We make this dedication for the benefit
of the public at large and to the detriment of our heirs and
successors. We intend this dedication to be an overt act of
relinquishment in perpetuity of all present and future rights to this
software under copyright law.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR ANY CLAIM, DAMAGES OR
OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
OTHER DEALINGS IN THE SOFTWARE.

For more information, please refer to <http://unlicense.org/>

---------------------------------------------------------------------------

Portions of this software derived from RFC 1320:

Copyright (C) 1990-2, RSA Data Security, Inc. All rights reserved.

License to copy and use this software is granted provided that it
is identified as the "RSA Data Security, Inc. MD4 Message-Digest
Algorithm" in all material mentioning or referencing this software
or this function.

License is also granted to make and use derivative works provided
that such works are identified as "derived from the RSA Data
Security, Inc. MD4 Message-Digest Algorithm" in all material
mentioning or referencing the derived work.

RSA Data Security, Inc. makes no representations concerning either
the merchantability of this software or the suitability of this
software for any particular purpose. It is provided "as is"
without express or implied warranty of any kind.

These notices must be retained in any copies of any part of this
documentation and/or software.

----------------------------------------------------------------------

The bundled llhttp dependency is licensed under the MIT license:

Copyright Fedor Indutny, 2018.

Permission is hereby granted, free of charge, to any person obtaining a
copy of this software and associated documentation files (the
"Software"), to deal in the Software without restriction, including
without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to permit
persons to whom the Software is furnished to do so, subject to the
following conditions:

The above copyright notice and this permission notice shall be included
in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN
NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE
USE OR OTHER DEALINGS IN THE SOFTWARE.
```

## 4. NOTICE.md

The rest of this file is `NOTICE.md` from the K2 source tree, unchanged.
Files it names by path are in the source tree. The licence texts it points
to under `resources/notices/` also ship in a `notices` folder beside this
file in the desktop app, the Arch package and the web app (the server .deb
carries no code they cover).

<!-- BEGIN NOTICE.md -->
# NOTICE

**K2 by Alakazam Labs** — AI workspace orchestration.
Copyright 2025-2026 Alakazam Labs.

## Licensing

K2 is licensed under the **MIT License**, from 0.46.10 on. See
[LICENSE.md](LICENSE.md). Releases up to and including 0.46.9 were
licensed under FSL-1.1-Apache-2.0 and stay under it; each of those
releases becomes available under Apache 2.0 two years after its release.

## Trademarks

"K2 by Alakazam Labs" and the K2 logo are trademarks of Alakazam Labs;
see [TRADEMARKS.md](TRADEMARKS.md). The MIT License does not grant
trademark rights.

## Third-party: Seti UI file icons (MIT)

The Files drawer uses the **Seti** icon font for file-type glyphs
(same family as VS Code’s default file icon theme).

| | |
|--|--|
| **What** | `seti.woff` + generated name/extension maps under `src/renderer/assets/seti/` and `src/renderer/lib/seti-file-icons/` |
| **Upstream** | [Seti UI](https://github.com/jesseweed/seti-ui) (Jesse Weed), packaged via VS Code `extensions/theme-seti` |
| **License** | **MIT** (Copyright 2014 Jesse Weed) — full text in `src/renderer/assets/seti/THIRD_PARTY_NOTICES.txt` |
| **K2 code** | Is **MIT**. MIT assets do **not** relicense the product. |
| **Not** | Not affiliated with Microsoft or Visual Studio Code. Do not use those trademarks for K2 branding. |

When redistributing K2 builds that include the Seti font, ship the MIT
notice (this section and/or `THIRD_PARTY_NOTICES.txt`).

## Third-party: LiteLLM model price table (MIT)

The Usage page's estimated API-equivalent cost uses per-token rates from
LiteLLM's `model_prices_and_context_window.json`.

| | |
|--|--|
| **What** | `crates/k2-core/data/llm_prices.json` (a trimmed copy, bundled) and the daemon's daily refresh into `~/.k2/usage/prices.json` |
| **Upstream** | [LiteLLM](https://github.com/BerriAI/litellm) (Berri AI) |
| **License** | **MIT** (Copyright (c) 2023 Berri AI) — full text in `crates/k2-core/data/LLM_PRICES_NOTICE.txt` |
| **K2 code** | Is **MIT**. The MIT data does **not** relicense the product. |

## Third-party: Lucide icons (ISC)

The top bar's icons, the tab Pin / Unpin, the Design tab palette, the
Design note tiles' edit, send and delete, the
Workspace drawer's phone push toggle, API section and Apps section, the setup checklist's rail button
(icon style A3) and the Mail page's folder and message-action icons are
Lucide icons,
copied in as SVG paths.
Home, Agents, Projects, Tickets, Wiki, Server, Admin profile, Run, Drawer,
the stopwatch clock, the Keep awake mug, Help, Pin, Unpin, the palette, the
pencil, the up arrow, the trash can, the checklist and
the smartphone (on, and off with a slash) are edited (corners squared: square caps, mitred
joins, no corner radius). Settings is Lucide's `settings` gear, unedited. The Gardens ensō
is K2's own (`ZenIcon`), not Lucide.

| | |
|--|--|
| **What** | SVG paths in `src/renderer/components/icons/K2Icons.tsx`, from `lucide-static@1.58.0/icons/` (`house`, `bot`, `layout-dashboard`, `ticket`, `library`, `settings`, `server`, `circle-user`, `play`, `panel-left`, `clock`, `coffee`, `circle-help`, `pin`, `pin-off`, `palette`, `smartphone`, `pencil`, `trash-2`, `arrow-up`, `list-todo`; for Mail `inbox`, `send`, `file-text`, `archive`, `octagon-alert`, `folder`, `folder-input`, `reply`, `reply-all`, `forward`, `star`, `flag`, `paperclip`, `refresh-cw`, `square-pen`, `mail-open`, `download`, `x`; the API section's `braces` and the Apps section's `app-window`, from `lucide-react@1.73.0`) |
| **Upstream** | [Lucide](https://github.com/lucide-icons/lucide) |
| **License** | **ISC** (Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of Feather (MIT). All other copyright (c) for Lucide are held by Lucide Contributors 2022.) — full text below |
| **K2 code** | Is **MIT**. The ISC icons do **not** relicense the product. |
| **Board widget** | Board's `crop` icon (`crates/k2-core/src/zen/builtin_widgets/board-1/icons.js`) is Lucide's; see the Board entry below. Board's other icons are its own drawing. |

> ISC License
>
> Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as
> part of Feather (MIT). All other copyright (c) for Lucide are held by
> Lucide Contributors 2022.
>
> Permission to use, copy, modify, and/or distribute this software for any
> purpose with or without fee is hereby granted, provided that the above
> copyright notice and this permission notice appear in all copies.
>
> THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
> WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
> MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
> ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
> WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
> ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
> OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

## Third-party: lucide-react, Home group icons (ISC)

The Home group icons and the All groups modal draw each group's icon from a
curated list of about 240 neutral Lucide icons (no brand or company logos),
imported by name from the `lucide-react` npm package and drawn squared
(square caps, mitred joins, stroke 1.75) like the icons above.

| | |
|--|--|
| **What** | npm `lucide-react` **1.73.0** (pinned exactly), the icons named in `src/renderer/lib/group-icons.ts` |
| **Upstream** | [Lucide](https://github.com/lucide-icons/lucide) (`packages/lucide-react`) |
| **License** | **ISC** (Copyright (c) 2026 Lucide Icons and Contributors), with the Feather-derived icons under **MIT** (Copyright (c) 2013-present Cole Bemis) — full text, unedited, in `src/renderer/assets/lucide/LICENSE` |
| **K2 code** | Is **MIT**. The ISC/MIT icons do **not** relicense the product. |

When redistributing K2 builds, ship this notice and/or
`src/renderer/assets/lucide/LICENSE`.

## Contributed: Board (Garden widget) — MIT

The Board Garden's whiteboard widget is **Board by Baden Ballard (MIT)**.

| | |
|--|--|
| **What** | `crates/k2-core/src/zen/builtin_widgets/board-1/` (the `k2:board@1` built-in widget) and `crates/k2-core/src/zen/builtin_widgets/board-file-1/` and `board-file-2/` (the Board tab for `.k5board` files), ported from Board's own widget; and Board code used by the Org Chart in `src/renderer/components/OrgChart/`: `org-camera.ts` and `OrgMinimap.tsx` (adapted from Board's camera and minimap) and `board-geom.ts` (which loads Board's `board-file-1/geom.js`) |
| **Author** | Baden Ballard |
| **License** | **MIT** (granted to K2 in writing by the author, 2026-11-01) |

Copyright (c) 2026 Baden Ballard. Licensed under MIT.

Board's `crop` icon is from Lucide (ISC). Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of Feather (MIT). All other copyright (c) for Lucide are held by Lucide Contributors 2022.

> ISC License
>
> Permission to use, copy, modify, and/or distribute this software for any purpose with or without fee is hereby granted, provided that the above copyright notice and this permission notice appear in all copies.
>
> THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

## Third-party: Public Suffix List (MPL-2.0) and BIP-39 word list (MIT)

The vault (`crates/k2-vault`) embeds two data files:

| | |
|--|--|
| **Public Suffix List** | `crates/k2-vault/data/public_suffix_list.dat`, from [publicsuffix.org](https://publicsuffix.org/list/public_suffix_list.dat), Mozilla Public License 2.0 (header kept in the file). Used to refuse wildcard hosts over shared suffixes. Refreshed each release. |
| **BIP-39 English word list** | `crates/k2-vault/data/bip39-english.txt`, from [bitcoin/bips](https://github.com/bitcoin/bips/blob/master/bip-0039/english.txt), MIT (per BIP-0039). Used for the 24-word vault recovery phrase. |
| **K2 code** | Is **MIT**. |

## Third-party: CLDR Windows time-zone mappings (Unicode-3.0)

The calendar code turns the Windows time-zone names Microsoft 365 sends
(`Pacific Standard Time`) into IANA names (`America/Los_Angeles`) with a
table generated from the Unicode CLDR.

| | |
|--|--|
| **What** | `crates/k2-daemon/src/mail/windows_zones.rs`: the territory-001 mappings of CLDR 49 `common/supplemental/windowsZones.xml`, generated by `scripts/gen-windows-zones.py` (only the mappings; regenerate from a newer CLDR release to update) |
| **Upstream** | [Unicode CLDR](https://github.com/unicode-org/cldr/blob/release-49/common/supplemental/windowsZones.xml) |
| **License** | **Unicode-3.0** (Copyright © 1991-2026 Unicode, Inc.), full text below |
| **K2 code** | Is **MIT**. The Unicode data does **not** relicense the product. |

```text
UNICODE LICENSE V3

COPYRIGHT AND PERMISSION NOTICE

Copyright © 1991-2026 Unicode, Inc.

NOTICE TO USER: Carefully read the following legal agreement. BY
DOWNLOADING, INSTALLING, COPYING OR OTHERWISE USING DATA FILES, AND/OR
SOFTWARE, YOU UNEQUIVOCALLY ACCEPT, AND AGREE TO BE BOUND BY, ALL OF THE
TERMS AND CONDITIONS OF THIS AGREEMENT. IF YOU DO NOT AGREE, DO NOT
DOWNLOAD, INSTALL, COPY, DISTRIBUTE OR USE THE DATA FILES OR SOFTWARE.

Permission is hereby granted, free of charge, to any person obtaining a
copy of data files and any associated documentation (the "Data Files") or
software and any associated documentation (the "Software") to deal in the
Data Files or Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, and/or sell
copies of the Data Files or Software, and to permit persons to whom the
Data Files or Software are furnished to do so, provided that either (a)
this copyright and permission notice appear with all copies of the Data
Files or Software, or (b) this copyright and permission notice appear in
associated Documentation.

THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS", WITHOUT WARRANTY OF ANY
KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF
THIRD PARTY RIGHTS.

IN NO EVENT SHALL THE COPYRIGHT HOLDER OR HOLDERS INCLUDED IN THIS NOTICE
BE LIABLE FOR ANY CLAIM, OR ANY SPECIAL INDIRECT OR CONSEQUENTIAL DAMAGES,
OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS,
WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,
ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THE DATA
FILES OR SOFTWARE.

Except as contained in this notice, the name of a copyright holder shall
not be used in advertising or otherwise to promote the sale, use or other
dealings in these Data Files or Software without prior written
authorization of the copyright holder.

SPDX-License-Identifier: Unicode-3.0
```

## Third-party: wry (Apache-2.0 OR MIT)

K2 vendors a patched copy of **wry** 0.57.0 under `third_party/wry` so a
nil `NSURL` (`URLWithString`, IPC, navigation, `window.open`) cannot abort
the macOS app. crates.io 0.57.0 still unwraps those.

| | |
|--|--|
| **What** | `third_party/wry` (crates.io wry 0.57.0 + nil-URL no-abort patch + macOS IPC from the main frame only + empty-URI-path guard until the Tauri 2.12.5 move) |
| **Upstream** | [wry](https://github.com/tauri-apps/wry) |
| **License** | **Apache-2.0 OR MIT** (Copyright (c) 2020-2023 Ngo Iok Ui & Tauri Programme within The Commons Conservancy) — full text in `third_party/wry/LICENSE-MIT`, `third_party/wry/LICENSE-APACHE`, and `third_party/wry/LICENSE.spdx` |
| **K2 code** | Is **MIT**. The vendored crate does **not** relicense the product. |

## Third-party: Sew native voice crates (MIT OR Apache-2.0; Apache-2.0)

K2's desktop app can carry voice calls with Sew's client crates (the `native-voice` build feature), from crates.io (their source is Sew's public mirror, [Alakazam-211/sew-crates](https://github.com/Alakazam-211/sew-crates)), pinned by version in `src-tauri/Cargo.toml` and `Cargo.lock`.

| | |
|--|--|
| **What** | `tauri-plugin-sew-voice`, `sew-call-client`, `sew-mls-crypto`, `sew-e2ee`, `sew-proto`, `sew-noise` (MIT OR Apache-2.0); `sew-lk-compat`, `sew-lkproto` (Apache-2.0) |
| **Upstream** | Alakazam Labs, Sew |
| **Notice** | `sew-lk-compat` and `sew-lkproto` hold code ported from or generated from LiveKit (Apache-2.0, Copyright LiveKit, Inc.). `sew-mls-crypto` holds code derived from openmls_rust_crypto (MIT, Copyright OpenMLS Authors). `sew-noise` (noise removal) links [nnnoiseless](https://github.com/jneem/nnnoiseless) 0.5.2, the Rust port of RNNoise, and through it compiles in RNNoise's model weights (BSD-3-Clause: Copyright (c) 2020 Joe Neeman; (c) 2017 Mozilla; (c) 2007-2017 Jean-Marc Valin; (c) 2005-2017 Xiph.Org Foundation; (c) 2003-2004 Mark Borgerding); its `src/dfn3/stream.rs` is ported from DeepFilterNet's libDF (MIT OR Apache-2.0, Copyright (c) 2021 Hendrik Schröter). Each crate's NOTICE ships in the app under `resources/notices/` (`sew-noise-NOTICE` carries nnnoiseless's full BSD-3-Clause text). Not affiliated with or endorsed by LiveKit, Inc. |
| **K2 code** | Is **MIT**. These crates do **not** relicense the product. |

## Third-party: DeepFilterNet3 model weights (MIT OR Apache-2.0)

Noise removal in calls (Settings → Voice → Noise removal, on by default) runs the DeepFilterNet3 model (low-latency variant). Its weights ship with the desktop app under `dfn3/` in the app's resources, beside their NOTICE, `LICENSE-MIT` and `LICENSE-APACHE`; they are not in this repository (`scripts/fetch-dfn3.sh` fetches the pinned Sew release asset and checks it).

| | |
|--|--|
| **What** | `dfn3_ll.nnef.tar`, DeepFilterNet3 (low-latency variant) model weights |
| **Upstream** | [DeepFilterNet](https://github.com/Rikorose/DeepFilterNet), the work of Hendrik Schröter and the DeepFilterNet authors. Copyright (c) 2021 Hendrik Schröter |
| **License** | Either the MIT license or the Apache License, Version 2.0, at your option (the authors' texts, unchanged, ship beside the weights) |
| **Changes** | Converted by Alakazam Labs. The file `dfn3_ll.nnef.tar` is a modified form of `DeepFilterNet3_ll_onnx.tar.gz` from the authors' repository (`models/`, commit `978576aa8400552a4ce9730838c635aa30db5e61`, the v0.5.6 tag): the ONNX graphs' float32 weights were renamed and stored as NNEF tensor files, values unchanged, with an NNEF graph that declares them; the graphs' operators are not included. Sew's noise removal (the `sew-noise` crate, cargo feature `dfn3`) runs this model with its own inference code. |
| **Citation** | The authors ask: if you use the DeepFilterNet3 model, please cite *DeepFilterNet: Perceptually Motivated Real-Time Speech Enhancement* (Schröter, Rosenkranz, Escalante-B., Maier; INTERSPEECH 2023). |
| **K2 code** | Is **MIT**. The weights do **not** relicense the product. |

## Third-party: postal-sdk (MIT)

K2's Postal support links **postal-sdk**, the agent-to-agent mail SDK, from
the public postal-bot repository, pinned by commit.

| | |
|--|--|
| **What** | the `postal-sdk` crate (`crates/postal-sdk`), pinned at `9da217afc59b68df744d030bf1b22ada9454bada` |
| **Upstream** | [postal-bot](https://github.com/Alakazam-211/postal-bot) |
| **License** | **MIT** (Copyright (c) 2026 Alakazam Labs) — text below |
| **K2 code** | Is **MIT**. The SDK does **not** relicense the product. |

```
MIT License

Copyright (c) 2026 Alakazam Labs

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Third-party: objc2 (MIT)

K2 vendors a patched copy of **objc2** 0.6.5 under `third_party/objc2` so a
null return from `sel_registerName` cannot abort the macOS app. crates.io
0.6.5 still expects that pointer. The crates.io crate ships no LICENSE file.

| | |
|--|--|
| **What** | `third_party/objc2` (crates.io objc2 0.6.5 + null-selector no-abort patch) |
| **Upstream** | [objc2](https://github.com/madsmtm/objc2) |
| **License** | **MIT** (Copyright (c) Mads Marquart) — full text in `third_party/objc2/LICENSE` |
| **K2 code** | Is **MIT**. The vendored crate does **not** relicense the product. |

## Third-party: tao (Apache-2.0)

K2 vendors a patched copy of **tao** 0.37.1 under `third_party/tao` so a null
superclass pointer is not dereferenced. A null `class_getSuperclass` does not
fall back to the receiver's own class. crates.io 0.37.1 still dereferences
that pointer.

| | |
|--|--|
| **What** | `third_party/tao` (crates.io tao 0.37.1 + null-superclass patch) |
| **Upstream** | [tao](https://github.com/tauri-apps/tao) |
| **License** | **Apache-2.0** (Copyright 2021-2023 Tauri Programme within The Commons Conservancy) — full text in `third_party/tao/LICENSE` and `third_party/tao/LICENSE.spdx` |
| **K2 code** | Is **MIT**. The vendored crate does **not** relicense the product. |

## Third-party: vte (Apache-2.0 OR MIT)

K2 vendors a patched copy of **vte** 0.15.0 (the terminal parser inside
alacritty_terminal) under `third_party/vte` so a UTF-8 character split
across two PTY reads cannot drop the ASCII byte that follows it. crates.io
0.15.0 still drops it.

| | |
|--|--|
| **What** | `third_party/vte` (crates.io vte 0.15.0 + split-UTF-8 byte-drop fix, `third_party/upstream/vte-0001-partial-utf8-ascii-drop.patch`) |
| **Upstream** | [vte](https://github.com/alacritty/vte) |
| **License** | **Apache-2.0 OR MIT** (Copyright (c) 2016 Joe Wilm) — full text in `third_party/vte/LICENSE-MIT` and `third_party/vte/LICENSE-APACHE` |
| **K2 code** | Is **MIT**. The vendored crate does **not** relicense the product. |

## Third-party: Mermaid (MIT)

The file viewer draws Mermaid diagrams with a vendored build of **Mermaid**.

| | |
|--|--|
| **What** | `src/renderer/public/vendor/mermaid.min.js` (Mermaid 11.16.1, upstream `dist/mermaid.min.js` byte for byte, loaded by `components/FileViewerPane/DiagramViewer.tsx`; its inlined DOMPurify is 3.4.0, as upstream built it) |
| **Upstream** | [mermaid](https://github.com/mermaid-js/mermaid) |
| **License** | **MIT** (Copyright (c) 2014 - 2022 Knut Sveidqvist) — full text in `src/renderer/public/vendor/mermaid.LICENSE.txt`. The prebuilt file also carries Mermaid's own dependencies (among them d3, dagre-d3-es, cytoscape, khroma, DOMPurify, KaTeX, marked), each under its own permissive licence. |
| **K2 code** | Is **MIT**. Mermaid does **not** relicense the product. |

## Third-party: CAD file previews (MIT)

The file viewer previews DXF drawings and 3D models (STL, OBJ, 3MF, PLY,
glTF/GLB) with these npm packages, bundled into lazy chunks that load only
when such a file opens.

| | |
|--|--|
| **What** | `three` 0.186.0 (core, OrbitControls, STL/OBJ/3MF/PLY/GLTF loaders, and the **fflate** 0.8.2 copy three ships for 3MF) · `dxf-parser` 1.1.2 (with its dependency `loglevel` 1.9.2). Used by `src/renderer/components/FileViewerPane/` (`DxfViewer.tsx`, `dxfModel.ts`, `ModelViewer.tsx`, `modelParse.ts`, `modelScene.ts`) |
| **Upstream** | [three.js](https://github.com/mrdoob/three.js) · [fflate](https://github.com/101arrowz/fflate) · [dxf-parser](https://github.com/gdsestimating/dxf-parser) · [loglevel](https://github.com/pimterry/loglevel) |
| **License** | **MIT** (Copyright © 2010-2026 three.js authors; Copyright (c) 2023 Arjun Barrett; Copyright (c) 2015 GDS Storefront Estimating; Copyright (c) 2013 Tim Perry) — full texts in `src/renderer/components/FileViewerPane/THIRD_PARTY_NOTICES.txt` |
| **K2 code** | Is **MIT**. These packages do **not** relicense the product. |

The same three.js version is also vendored for Garden widgets (the Zen
widget library below). That copy is a pre-bundled classic script for sealed
widget frames, ships with the desktop app only, and lacks the 3MF and PLY
loaders, so the file viewer bundles its own ES-module copy.

## Third-party: CodeMirror merge view (MIT)

The Changes tab's diff viewer (inline and side-by-side) uses
`@codemirror/merge`, bundled into a lazy chunk that loads only when a
Changes tab shows a diff.

| | |
|--|--|
| **What** | `@codemirror/merge` 6.12.2, used by `src/renderer/components/Changes/viewer/` |
| **Upstream** | [codemirror/merge](https://github.com/codemirror/merge) |
| **License** | **MIT** (Copyright (C) 2018-2022 by Marijn Haverbeke and others) — full text in the package's `LICENSE` |
| **K2 code** | Is **MIT**. This package does **not** relicense the product. |

## Third-party: shiki grammars under the TextMate bundle grant and MPL-2.0

The code viewer highlights code with [shiki](https://github.com/shikijs/shiki)
(MIT) and its grammar collection, tm-grammars (MIT), which carries each
language's grammar from its own repository at a pinned commit. Eleven of
those grammars are under neither MIT, Apache-2.0 nor BSD: eight TextMate
bundles under the TextMate bundle grant (the licence section of each bundle's
README) and three under the Mozilla Public License 2.0. K2 does not edit
them, but they don't ship as the upstream files: tm-grammars converts each
grammar to JSON (a `.tmLanguage` plist included), shiki's `@shikijs/langs`
wraps that JSON in a JavaScript module, and the app's build bundles and
minifies the module into its JavaScript. The conversion changes the content as
well as the format: it drops comments and metadata fields, sets the grammar's
name to shiki's id and adds a display name and aliases, and rewrites the
regular expressions into shorter forms (`[0-9a-fA-F]` becomes `\h`, for
example). For the three MPL-2.0 grammars, the Source Code Form (MPL-2.0
section 3.2) is the upstream file at the URL and commit in the table, which
anyone can get there; it also ships in the app under `resources/notices/`,
byte for byte (the file and its sha256 in the row). Each grammar's licence
text ships in the app under `resources/notices/` too, byte for byte from its
repository at that commit (the file and its sha256 below). The grammars whose
licences don't allow shipping are left out of the bundle:
`scripts/notices/shiki-excluded.txt`.

| Grammar | Upstream, at the pinned commit | Licence, and where its text is | Text (`resources/notices/`) | sha256 |
|--|--|--|--|--|
| `abap` | [pvl/abap.tmbundle/Syntaxes/ABAP.tmLanguage](https://github.com/pvl/abap.tmbundle/blob/92e17e62a9449d2a6f87bfb30b4f920986c24018/Syntaxes/ABAP.tmLanguage) at `92e17e62a9449d2a6f87bfb30b4f920986c24018` | TextMate bundle grant: `README.md`, its License section to the end of the file | `shiki-abap-LICENSE` | `9638e6747d5a8aed56316a5ced9ff1ddfa91614fdd451d491519b90a6eb349c3` |
| `applescript` | [textmate/applescript.tmbundle/Syntaxes/AppleScript.tmLanguage](https://github.com/textmate/applescript.tmbundle/blob/4947cad7494eba084c21cfe04689b4efce96700f/Syntaxes/AppleScript.tmLanguage) at `4947cad7494eba084c21cfe04689b4efce96700f` | TextMate bundle grant: `README.mdown`, its License section to the end of the file | `shiki-applescript-LICENSE` | `192c2900d6b509c3f962c377e0c46b0bb39e8615c2b97170bb6607904c2f1ff5` |
| `erb` | [textmate/ruby.tmbundle/Syntaxes/HTML (Ruby - ERB).tmLanguage](https://github.com/textmate/ruby.tmbundle/blob/ecf0f1df23984bcff9b51552e35d9696cb0d9539/Syntaxes/HTML%20%28Ruby%20-%20ERB%29.tmLanguage) at `ecf0f1df23984bcff9b51552e35d9696cb0d9539` | TextMate bundle grant: `README.mdown`, its License section to the end of the file | `shiki-erb-LICENSE` | `192c2900d6b509c3f962c377e0c46b0bb39e8615c2b97170bb6607904c2f1ff5` |
| `logo` | [textmate/logo.tmbundle/Syntaxes/Logo.tmLanguage](https://github.com/textmate/logo.tmbundle/blob/3205e987ed2a2aa11accdd55a7d094832441f89c/Syntaxes/Logo.tmLanguage) at `3205e987ed2a2aa11accdd55a7d094832441f89c` | TextMate bundle grant: `README.mdown`, its License section to the end of the file | `shiki-logo-LICENSE` | `192c2900d6b509c3f962c377e0c46b0bb39e8615c2b97170bb6607904c2f1ff5` |
| `mipsasm` | [textmate/mips.tmbundle/Syntaxes/MIPS.tmLanguage](https://github.com/textmate/mips.tmbundle/blob/eb7601016a109bf912a48a1c21988f3c4a59fb3a/Syntaxes/MIPS.tmLanguage) at `eb7601016a109bf912a48a1c21988f3c4a59fb3a` | TextMate bundle grant: `README.mdown`, its License section to the end of the file | `shiki-mipsasm-LICENSE` | `192c2900d6b509c3f962c377e0c46b0bb39e8615c2b97170bb6607904c2f1ff5` |
| `po` | [textmate/gettext.tmbundle/Syntaxes/Gettext.tmLanguage](https://github.com/textmate/gettext.tmbundle/blob/08bb69558d82ada8d4f36b8869f871ce69014749/Syntaxes/Gettext.tmLanguage) at `08bb69558d82ada8d4f36b8869f871ce69014749` | TextMate bundle grant: `README.mdown`, its License section to the end of the file | `shiki-po-LICENSE` | `192c2900d6b509c3f962c377e0c46b0bb39e8615c2b97170bb6607904c2f1ff5` |
| `ssh-config` | [textmate/ssh-config.tmbundle/Syntaxes/SSH-Config.tmLanguage](https://github.com/textmate/ssh-config.tmbundle/blob/212a0000c50e0c877303b2c4fcead539d2af2670/Syntaxes/SSH-Config.tmLanguage) at `212a0000c50e0c877303b2c4fcead539d2af2670` | TextMate bundle grant: `README.mdown`, its License section to the end of the file | `shiki-ssh-config-LICENSE` | `192c2900d6b509c3f962c377e0c46b0bb39e8615c2b97170bb6607904c2f1ff5` |
| `toml` | [textmate/toml.tmbundle/Syntaxes/TOML.tmLanguage](https://github.com/textmate/toml.tmbundle/blob/e82b64c1e86396220786846201e9aa3f0a2d9ca2/Syntaxes/TOML.tmLanguage) at `e82b64c1e86396220786846201e9aa3f0a2d9ca2` | TextMate bundle grant: `README.mdown`, its License section to the end of the file | `shiki-toml-LICENSE` | `192c2900d6b509c3f962c377e0c46b0bb39e8615c2b97170bb6607904c2f1ff5` |
| `bird2` | [bird-chinese-community/BIRD-tm-language-grammar/grammars/bird2.tmLanguage.json](https://github.com/bird-chinese-community/BIRD-tm-language-grammar/blob/1aa378ed0add766bee2271acece94d5068cf3de9/grammars/bird2.tmLanguage.json) at `1aa378ed0add766bee2271acece94d5068cf3de9` | MPL-2.0: `LICENSE` (the repository's `LICENSE.sample`, an MIT text, is not the grammar's licence); the Source Code Form, `grammars/bird2.tmLanguage.json` at that commit, ships as `shiki-bird2-SOURCE.tmLanguage.json` (sha256 `e84a57ccc9f5b6236e9d3f7fa82c62e89dea37b8cb94d02db7eb907a46412e7b`); tm-grammars' converted form, the one the app is built from (`grammars/bird2.json` in the npm package `tm-grammars@1.31.6`, integrity `sha512-V2nz1Z09xiUjpN2ILRw8jlpOoIN1ZtAfAqXawRZi/aPbA1Q3IEl9VnjCKOHLqCPxXv/HAL6zBml2OcxWa2wd6A==`; its home is [shikijs/textmate-grammars-themes](https://github.com/shikijs/textmate-grammars-themes)), ships as `shiki-bird2-CONVERTED-tm-grammars-1.31.6.json` (sha256 `c78f5a4801bd647434f0dbb90fa4075aa8a8fabe17c9af8155baa052147e75cc`) | `shiki-bird2-LICENSE` | `9ee744951d725ad1911f95d1021b37f5713e94e9fffc84448dc1ffd597ad17aa` |
| `hcl` | [hashicorp/syntax/syntaxes/hcl.tmGrammar.json](https://github.com/hashicorp/syntax/blob/2ec7cd10c9d6134e836bf85eb0f5aa010512bff4/syntaxes/hcl.tmGrammar.json) at `2ec7cd10c9d6134e836bf85eb0f5aa010512bff4` | MPL-2.0: `LICENSE`, the same file at both hashicorp/syntax commits; the Source Code Form, `syntaxes/hcl.tmGrammar.json` at that commit, ships as `shiki-hcl-SOURCE.tmGrammar.json` (sha256 `5a2e52e505fc3e4a87a34bfb39cb42e7306f00f60d0ae2521755591e70526e93`); tm-grammars' converted form, the one the app is built from (`grammars/hcl.json` in the npm package `tm-grammars@1.31.6`, integrity `sha512-V2nz1Z09xiUjpN2ILRw8jlpOoIN1ZtAfAqXawRZi/aPbA1Q3IEl9VnjCKOHLqCPxXv/HAL6zBml2OcxWa2wd6A==`; its home is [shikijs/textmate-grammars-themes](https://github.com/shikijs/textmate-grammars-themes)), ships as `shiki-hcl-CONVERTED-tm-grammars-1.31.6.json` (sha256 `8ad2847f1765adbe564202c7bf0226ffa7a11c3e14c3cf9ff79812c69d250a1b`) | `shiki-hcl-LICENSE` | `eeee9e927d8039479976f714644a09bb4db8e23c3ac287d75446e5ebdb9b25c3` |
| `terraform` | [hashicorp/syntax/syntaxes/terraform.tmGrammar.json](https://github.com/hashicorp/syntax/blob/cc2b4d4fe389f14b8a13937f4e0d7b2811b57588/syntaxes/terraform.tmGrammar.json) at `cc2b4d4fe389f14b8a13937f4e0d7b2811b57588` | MPL-2.0: `LICENSE`, the same file at both hashicorp/syntax commits; the Source Code Form, `syntaxes/terraform.tmGrammar.json` at that commit, ships as `shiki-terraform-SOURCE.tmGrammar.json` (sha256 `63b2c259398559c41e6479ef016a85eb345eab0bcee207064d06ae4332961344`); tm-grammars' converted form, the one the app is built from (`grammars/terraform.json` in the npm package `tm-grammars@1.31.6`, integrity `sha512-V2nz1Z09xiUjpN2ILRw8jlpOoIN1ZtAfAqXawRZi/aPbA1Q3IEl9VnjCKOHLqCPxXv/HAL6zBml2OcxWa2wd6A==`; its home is [shikijs/textmate-grammars-themes](https://github.com/shikijs/textmate-grammars-themes)), ships as `shiki-terraform-CONVERTED-tm-grammars-1.31.6.json` (sha256 `3c4984f932c6c9d672505b737e75fb6612b1849ca32cffd62efaf731babb6c6e`) | `shiki-terraform-LICENSE` | `eeee9e927d8039479976f714644a09bb4db8e23c3ac287d75446e5ebdb9b25c3` |

K2 code is **MIT**. These grammars do **not** relicense the product.

## Third-party: T3 Code device-frame numbers (MIT)

The Mobile tab's phone frame is K2's own CSS. Its bezel and corner-radius
fractions per device kind, and the fit rule (fit by height, then clamp to
the width left beside the control rail), are taken from T3 Code. No T3
Code device models or other assets are used; Apple's 3D device models are
not used at all.

| | |
|--|--|
| **What** | Shape numbers from `packages/client-runtime/src/device/shapeProfile.ts` and `fitDeviceFrame` from `apps/web/src/components/device/deviceFrameLayout.ts`, adapted in `src/renderer/components/Mobile/device-frame.ts` |
| **Upstream** | [pingdotgg/t3code](https://github.com/pingdotgg/t3code) |
| **License** | **MIT** (Copyright (c) 2026 T3 Tools Inc.) — full text below |
| **K2 code** | Is **MIT**. This does **not** relicense the product. |

> MIT License
>
> Copyright (c) 2026 T3 Tools Inc.
>
> Permission is hereby granted, free of charge, to any person obtaining a copy
> of this software and associated documentation files (the "Software"), to deal
> in the Software without restriction, including without limitation the rights
> to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
> copies of the Software, and to permit persons to whom the Software is
> furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all
> copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
> IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
> FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
> AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
> LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
> OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
> SOFTWARE.

## Third-party: Wiki 3D view (MIT, BSD-3-Clause)

The Wiki page's 3D map uses these npm packages, bundled into a lazy chunk
that loads only when the 3D view opens (three.js is shared with the file
viewer above).

| | |
|--|--|
| **What** | `react-force-graph-3d` 1.29.2 · `3d-force-graph` 1.80.1 · `three-forcegraph` 1.43.6 · `three-render-objects` 1.43.0 · `three-spritetext` 1.10.0 · `data-bind-mapper` 1.0.3 · `@tweenjs/tween.js` 25.0.0 · `polished` 4.3.1 (+ `@babel/runtime` helpers) · `ngraph.forcelayout` 3.3.1 · `ngraph.graph` 20.1.2 · `ngraph.events` 1.4.0 · `ngraph.random` 1.2.0 · `ngraph.merge` 1.0.0. Used by `src/renderer/components/Wiki/WikiGraph3D.tsx` and `wiki-3d-scene.ts` |
| **Upstream** | [react-force-graph](https://github.com/vasturiano/react-force-graph) · [3d-force-graph](https://github.com/vasturiano/3d-force-graph) · [three-spritetext](https://github.com/vasturiano/three-spritetext) · [ngraph](https://github.com/anvaka/ngraph.forcelayout) · [polished](https://github.com/styled-components/polished) · [tween.js](https://github.com/tweenjs/tween.js) |
| **License** | **MIT** (Copyright (c) 2017-2024 Vasco Asturiano; Brian Hough and Maximilian Stoiber; Sebastian McKenzie and other contributors; Tween.js authors; Andrei Kashcha for ngraph.merge) and **BSD-3-Clause** for ngraph.forcelayout, ngraph.graph, ngraph.events and ngraph.random (Copyright (c) Andrei Kashcha) — full texts in `src/renderer/components/Wiki/THIRD_PARTY_NOTICES.txt` |
| **K2 code** | Is **MIT**. These packages do **not** relicense the product. |

## Multiplayer kit: k2-mpc-base v0.24.0 (vendored)

Live docs (Markdown, plain text and any file) run on Alakazam Labs' multiplayer kit,
vendored with `git subtree`.

| | |
|--|--|
| **What** | `vendor/k2-mpc-base` (sessions, presence, store, disk sync, history, blame, erasure, text tools, parity, the export contract, the structure index, comments and claims by node), with its own `NOTICE.md` and `CHANGELOG.md`; and its JS client `k2-mpc-client` 0.24.0 (`vendor/k2-mpc-base/client/src`: the library under the app's Live editor, `src/renderer/components/LiveEditor`) |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-base`, `v0.24.0`, its cut `1ddeb73` (crate version `0.24.0`, client `0.24.0`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE` |

Its Rust dependencies, in K2's `Cargo.lock`:

| Crate | Version | Licence | Used for |
|---|---|---|---|
| [yrs](https://github.com/y-crdt/y-crdt) | 0.28.0 | MIT | The CRDT engine (Yjs-compatible), private to the kit; a modified copy vendored inside the kit (`vendor/k2-mpc-base/vendor/yrs`, a path dependency since kit v0.9.3), with its licence and the changes listed in the kit's `NOTICE.md` |
| [regex-automata](https://github.com/rust-lang/regex) | 0.4 | MIT OR Apache-2.0 | Linear-time regexes for the text tools (`grep`, `sed`); already in the tree |
| [redb](https://github.com/cberner/redb) | 2 | MIT OR Apache-2.0 | The docs store (`~/.k2/docs.redb`); the copy k2-core already links |
| thiserror | 2 | MIT OR Apache-2.0 | Error types |
| [libc](https://github.com/rust-lang/libc) | 0.2 | MIT OR Apache-2.0 | Linux (glibc) only: returning freed pages to the OS after a cut; already in the tree |
| dashmap (via yrs) | 6.2.1 | MIT | |
| smallstr (via yrs) | 0.3.1 | MIT OR Apache-2.0 | |

`unicode-ident` is `(MIT OR Apache-2.0) AND Unicode-3.0`, used only at build
time by procedural macros; Unicode-3.0 is OSI-approved and permissive.

The JS client adds no npm dependency, and since client 0.12.0 it has no peer
dependencies either: it is a library with no DOM. The Live editor is K2's own code, on
the app's own `@codemirror/*` packages (`view`, `state`, `commands`, `language`,
`lang-markdown`; MIT).

## Multiplayer kit: k2-mpc-markdown v0.7.28 (vendored)

The Markdown type pack for the kit (schema, file codec, its own verbs such as
`replace-section`, the parity list), vendored beside it.

| | |
|--|--|
| **What** | `vendor/k2-mpc-markdown` (depends on `../k2-mpc-base/crates/base`), with its own `NOTICE.md` and `CHANGELOG.md` |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-markdown`, tag `v0.7.28` (`e2d0ee2`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE` |
| **Dependencies** | k2-mpc-base only (above): yrs (MIT), regex-automata, redb and thiserror (MIT OR Apache-2.0), and `unicode-ident` (build time only, Unicode-3.0) |

## Multiplayer kit: k2-mpc-text v0.2.30 (vendored)

The plain-text type pack for the kit (code, config, markup, diagram sources,
`.txt`, and `.csv`/`.tsv` as text: a per-extension file-type table, a
byte-exact codec, the text tier's parity list), vendored beside it.

| | |
|--|--|
| **What** | `vendor/k2-mpc-text` (depends on `../k2-mpc-base/crates/base`), with its own `NOTICE.md` and `CHANGELOG.md` |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-text`, tag `v0.2.30` (`5d433e0`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE` |
| **Dependencies** | k2-mpc-base (above) and serde_json (MIT OR Apache-2.0; strict JSON for the merges of JSON types, already in K2's tree); its tests use proptest (MIT OR Apache-2.0) |

## Multiplayer kit: k2-mpc-board v0.1.18 (vendored)

The `.k5board` file format and its pack (a strict zip reader and writer,
canonical JSON, typed refusals, edits with per-object revisions, the agent
verbs, a three-way merge, and a bridge to the kit's verb features), vendored
beside the kit.

| | |
|--|--|
| **What** | `vendor/k2-mpc-board`, with its own `NOTICE.md`, `THIRD-PARTY-NOTICES.md` and `CHANGELOG.md`. Its `fuzz/` workspace isn't built by K2 |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-board`, tag `v0.1.18` (`4e01339`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE` |
| **Credit** | Parts of this crate come from Board, Copyright (c) 2026 Baden Ballard, MIT: the schema tables, the templates, the verbs' rules, the parity list, and the shapes the widget conversion pairs describe. See `THIRD-PARTY-NOTICES.md` for which files, and the licence. |
| **Dependencies** | miniz_oxide (MIT OR Zlib OR Apache-2.0), sha2 (MIT OR Apache-2.0); its `bridge/` adds the kit (k2-mpc-base, above) |

## Multiplayer kit: k2-mpc-psd v0.1.13 (vendored)

The kit's pack that reads PSD and PSB files: each version is read into the
kit's structure index (a tree of layers), streaming, never holding pixels; it
never writes a file. Vendored beside the kit from its public cut, which has no
`fuzz/` and no test-only checker scripts.

| | |
|--|--|
| **What** | `vendor/k2-mpc-psd` (depends on `../k2-mpc-base/crates/base`), with its own `NOTICE.md` and `CHANGELOG.md` |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-psd`, tag `v0.1.13` (the cut `dde1623`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE` |
| **Credit** | Its file reader is a copy of the PSD crate of PhotoCraft 0.9.0 (Copyright (c) 2026 ArtCraft Team and the PhotoCraft contributors, MIT OR Apache-2.0), with the changes listed in `vendor/k2-mpc-psd/vendor/photocraft-psd/VENDORED.md` (since v0.1.2 also one change from upstream commit `a7301f1d`: the `UnFl` count's reservation cap); its licence texts and `NOTICE` are beside it |
| **Dependencies** | flate2 (MIT OR Apache-2.0, with miniz_oxide: MIT OR Zlib OR Apache-2.0), thiserror (MIT OR Apache-2.0), and the kit (above) |

## Multiplayer kit: k2-mpc-ooxml v0.1.2 (vendored)

The package layer the kit's office-file packs share (docx, pptx and xlsx): a bounded zip
reader and byte-exact writer, content types, relationships, and a three-way package merge.
Vendored beside the kit from its public cut, which has no `fuzz/` and no external test
checkers.

| | |
|--|--|
| **What** | `vendor/k2-mpc-ooxml` (the crate `k2-mpc-opc` 0.1.2), with its own `NOTICE.md` and `CHANGELOG.md`. `corpus/` is a manifest of test files fetched for a test run, never committed or built |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-ooxml`, tag `v0.1.2` (the cut `306d393`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE` |
| **Dependencies** | roxmltree 0.21 (MIT OR Apache-2.0), flate2 (MIT OR Apache-2.0, with miniz_oxide: MIT OR Zlib OR Apache-2.0), crc32fast (MIT OR Apache-2.0) |

## Multiplayer kit: k2-mpc-pptx v0.1.11 (vendored)

The kit's pack for `.pptx` decks: a deck as a live keyed doc (slides and shapes by their own
ids), agent verbs, and the file written back by splicing only what changed into its own
bytes. In K2 it runs behind the Slides core (Settings → Cores → Multiplayer). Vendored beside
the kit from its public cut, which has no `fuzz/`.

| | |
|--|--|
| **What** | `vendor/k2-mpc-pptx` (depends on `../k2-mpc-base/crates/base` and `../k2-mpc-ooxml/crates/opc`), with its own `NOTICE.md` and `CHANGELOG.md` |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-pptx`, tag `v0.1.11` (the cut `47c7060`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE` |
| **Dependencies** | the kit and k2-mpc-opc (above), and roxmltree 0.21 (MIT OR Apache-2.0) |
| **Formats** | docx, pptx and xlsx are file formats specified by ECMA-376 and ISO/IEC 29500. The pack is not affiliated with, endorsed by or sponsored by the formats' original vendor |

## Multiplayer kit: k2-mpc-timeline v0.1.3 (vendored)

The kit's pack for video timelines: OTIO, FCPXML and PRPROJ files are read into the kit's
structure index, and an OTIO file can also be a live keyed doc written back to its own bytes.
Vendored whole beside the kit (its `fuzz/` crate is test tooling K2 doesn't build). K2 builds it
into the daemon and the export worker, behind the Timeline core (off by default).

| | |
|--|--|
| **What** | `vendor/k2-mpc-timeline` (depends on `../k2-mpc-base/crates/base`), with its own `NOTICE.md` and `CHANGELOG.md` |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-timeline`, tag `v0.1.3` (`8c729e2`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE`. It holds no code copied from another project |
| **Dependencies** | the kit (above), thiserror 2, roxmltree 0.21 (with memchr: Unlicense OR MIT) and flate2 1 (MIT OR Apache-2.0) |
| **Formats** | OTIO is the file format of OpenTimelineIO, a project of the Academy Software Foundation. FCPXML and PRPROJ are the file formats of other companies' video editing products. This project is not affiliated with, endorsed by or sponsored by any of them; their names and marks belong to their owners and are used here only to name the file formats |

## Multiplayer kit: k2-mpc-fab v0.1.3 (vendored)

The kit's pack for circuit board fabrication files: photoplot and drill files are read into the
kit's structure index (read only; no writer). Vendored whole beside the kit (its `fuzz/` crate is
test tooling K2 doesn't build). K2 builds it into the daemon and the export worker, behind the
PCB fabrication core (off by default).

| | |
|--|--|
| **What** | `vendor/k2-mpc-fab` (depends on `../k2-mpc-base/crates/base`), with its own `NOTICE.md` and `CHANGELOG.md` |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-fab`, tag `v0.1.3` (`058dfbd`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE`. It holds no code copied from another project, and no text, tables or figures from the specifications it reads: it cites their section numbers only (the *Gerber Layer Format Specification*, revision 2026.05, and the *XNC Format Specification*, revision 2021.11) |
| **Dependencies** | the kit only (above) |
| **Formats** | Gerber Format is a registered trademark of Ucamco NV; the photoplot reader reads files in that format and is not affiliated with, endorsed by or sponsored by Ucamco. XNC is Ucamco's subset of the IPC-NC-349 drill format; Excellon is a trademark of its owner. These names are used here only to name the file formats |

## Multiplayer kit: k2-mpc-blend v0.3.2 (vendored)

The kit's pack for BLEND files (3D scenes): a file is read into the kit's structure index, its
meshes and cameras can be read for previews, and a file can also be a live keyed doc (objects'
placement, visibility and names) written back to its own bytes. Vendored whole beside the kit
(its `fuzz/` crate is test tooling K2 doesn't build). K2 builds it into the daemon and the export
worker, behind the 3D scene core (off by default).

| | |
|--|--|
| **What** | `vendor/k2-mpc-blend` (depends on `../k2-mpc-base/crates/base`), with its own `NOTICE.md`, `PROVENANCE.md` and `CHANGELOG.md` |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-blend`, tag `v0.3.2` (`ea70a55`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE`. It holds no code copied from another project; `PROVENANCE.md` says what it was written from |
| **Dependencies** | the kit (above), thiserror 2 and flate2 1 (MIT OR Apache-2.0), and ruzstd 0.9 (MIT, with twox-hash 2: MIT; files saved zstd-compressed, in pure Rust) |
| **Formats** | BLEND is the file format of Blender, a project of the Blender Foundation. "Blender" is a trademark of its owner, named here only to say whose file format this is. This project is not affiliated with, endorsed by or sponsored by it |

## Multiplayer kit: k2-mpc-cad v0.5.0 (vendored)

The kit's packs for 2D drawings: a format-neutral drawing model (`k2-mpc-cad-model`), the ASCII
DXF reader (`k2-mpc-dxf`, every version from R12 to 2018, into the kit's structure index, with its
edit verbs: attribute and text values, an entity's layer, colour or linetype, move, rotate, scale,
add, insert a block, delete), the DWG
reader (`k2-mpc-dwg`, the 2004 to 2018 family, read-only), and the drawing-model read a viewer
draws from. Vendored whole beside the kit (its `fuzz/` crate, `cad-measure` and the
`scripts/referee` checkers are test tooling K2 doesn't build or run). K2 builds it into the daemon
and the export worker, behind the CAD core (off by default): people and agents change a `.dxf`
through its edit verbs; nobody saves a drawing whole through K2, and a `.dwg` is read only.

| | |
|--|--|
| **What** | `vendor/k2-mpc-cad` (depends on `../k2-mpc-base/crates/base`), with its own `NOTICE.md`, `PROVENANCE.md` and `CHANGELOG.md` |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-cad`, tag `v0.5.0` (`3ceb6ce`) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE`. It holds no code copied from another project, and one table of data taken from one (below); `PROVENANCE.md` says what each part was written from. Its tables are written in its own words from the DXF reference and the Open Design Specification for .dwg files, read as sources of facts; no text or tables are copied from them |
| **Dependencies** | the kit only (above) |
| **Test data** | The test drawings are generated by the pack's own generator (`k2_mpc_dxf::testgen`) when the tests run. One DWG file is committed: `vendor/k2-mpc-cad/crates/dwg/tests/data/draw_sampler.dwg`, the generator's own sampler drawing as a 2018 DWG, made once by Cores' own test tooling (not by any vendor's software) and cleared by its sha256; its `MANIFEST.md` beside it and the pack's `PROVENANCE.md` say what it is and where it came from. No sample drawing from anyone else is committed. K2 doesn't build or ship it: it is a test input of the pack's own suite |
| **Formats** | DXF and DWG are trademarks of their owner. This pack reads files in the DXF and DWG formats and edits DXF files through its edit verbs; it writes no DWG. It is not affiliated with, endorsed by or sponsored by that owner, and makes no compatibility claim. The names are used here only to name the file formats |

**The colour-number palette.** The 256 RGB values of
`vendor/k2-mpc-cad/crates/cad-model/src/draw/aci.rs` (`TABLE`) are taken, as data, from ezdxf's
model-space palette (`DXF_DEFAULT_COLORS` in `src/ezdxf/colors.py`, ezdxf 1.4.2), under its
licence:

```
MIT License

Copyright (c) 2020 Manfred Moitzi

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Multiplayer kit: k2-mpc-av v0.3.0 (vendored)

The kit's reading of a set of AV system drawings over the CAD pack's drawing model: devices
across sheets, cables and the boundaries they are pulled through, off-sheet flag pairs, title
blocks and the sheet index. It never writes a drawing. Each project's conventions come from a
configuration file the caller passes; nothing about a project is built in. K2 builds it into the
daemon, behind the CAD core (off by default).

| | |
|--|--|
| **What** | `vendor/k2-mpc-av` (depends on `../k2-mpc-cad` and `../k2-mpc-base`), with its own `NOTICE.md`, `PROVENANCE.md` and `CHANGELOG.md` |
| **Upstream** | Alakazam Labs (K2 Cores), `Alakazam-211/k2-mpc-av`, tag `v0.3.0` (`bb940ab`, paired with k2-mpc-cad v0.5.0) |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 Alakazam Labs), full texts in the vendored repo's `LICENSE-MIT` and `LICENSE-APACHE`. It holds no code copied from another project; `PROVENANCE.md` says what each part was written from |
| **Dependencies** | the kit and the CAD pack (above) |
| **Formats** | DXF and DWG are trademarks of their owner; they are named here only to name the file formats |

Update these sections on every kit bump.

## Third-party: deckcraft v0.18.1, for .pptx previews (MIT OR Apache-2.0, vendored)

K2 draws previews of `.pptx` slides with the PPTX reader and the CPU renderer of
**DeckCraft**, vendored (a `git subtree`, squashed), run only inside the separate
`k2-deckrender` process.

| | |
|--|--|
| **What** | `vendor/deckcraft`: the crates deckcraft-pptx and deckcraft-render and the in-repo crates they need (anim, color, fonts, geom, metafile, model, text), with upstream's `NOTICE` (trimmed) and `VENDORED.md` |
| **Upstream** | `storytold/deckcraft v0.18.1 (7cc73ab)`: [storytold/deckcraft](https://github.com/storytold/deckcraft), tag `v0.18.1`, commit `7cc73ab1895ed2e30c8afbcada20c70b9ea0a440` |
| **License** | **MIT OR Apache-2.0** (Copyright (c) 2026 ArtCraft Team and the DeckCraft contributors), full texts in `vendor/deckcraft/LICENSE-MIT` and `vendor/deckcraft/LICENSE-APACHE` |
| **Local changes** | Listed in `vendor/deckcraft/VENDORED.md`, and each changed file says so on its first line: the last-resort font is Liberation Sans 2.1.5 (below) in place of `epaint_default_fonts`, and craft-fonts input is refused; zip 9 in place of zip 3; the workspace keeps only these crates, with no git sources and no `[patch]`; the tests that need deckcraft-engine (not vendored) are removed |
| **Not included** | deckcraft-engine, the CLI and app, deckcraft-media and the codec crates, and every asset and brand file. K2 is not affiliated with or endorsed by the ArtCraft Team |
| **K2 code** | Is **MIT**. The vendored crates do **not** relicense the product. |

## Third-party: Liberation Sans 2.1.5 (SIL Open Font License 1.1)

The `.pptx` previews' last-resort font, compiled into `k2-deckrender` (through the vendored
deckcraft-fonts): the four Liberation Sans styles of liberation-fonts **2.1.5**, unmodified and
whole.

| | |
|--|--|
| **What** | `vendor/deckcraft/crates/fonts/liberation-fonts-ttf-2.1.5/` (`LiberationSans-Regular.ttf`, `-Bold.ttf`, `-Italic.ttf`, `-BoldItalic.ttf`, with upstream's `LICENSE`, `AUTHORS` and `ChangeLog`; every file pinned by sha256 in `SHA256SUMS` there) |
| **Upstream** | [liberationfonts/liberation-fonts](https://github.com/liberationfonts/liberation-fonts), release 2.1.5 (`liberation-fonts-ttf-2.1.5.tar.gz`, sha256 `7191c669bf38899f73a2094ed00f7b800553364f90e2637010a69c0e268f25d0`) |
| **Copyright** | Digitized data copyright (c) 2010 Google Corporation, with Reserved Font Arimo, Tinos and Cousine. Copyright (c) 2012 Red Hat, Inc., with Reserved Font Name Liberation. |
| **License** | **SIL Open Font License, Version 1.1**, full text in `vendor/deckcraft/crates/fonts/liberation-fonts-ttf-2.1.5/LICENSE` and below. The fonts are not renamed, subset or modified; "Liberation" is a Reserved Font Name |
| **K2 code** | Is **MIT**. The font files keep their own licence and do **not** relicense the product. |

<details>
<summary>SIL Open Font License, Version 1.1</summary>

```
Digitized data copyright (c) 2010 Google Corporation
	with Reserved Font Arimo, Tinos and Cousine.
Copyright (c) 2012 Red Hat, Inc.
	with Reserved Font Name Liberation.

This Font Software is licensed under the SIL Open Font License,
Version 1.1.

This license is copied below, and is also available with a FAQ at:
http://scripts.sil.org/OFL

SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007

PREAMBLE The goals of the Open Font License (OFL) are to stimulate
worldwide development of collaborative font projects, to support the font
creation efforts of academic and linguistic communities, and to provide
a free and open framework in which fonts may be shared and improved in
partnership with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves.
The fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works.  The fonts and derivatives,
however, cannot be released under any other type of license.  The
requirement for fonts to remain under this license does not apply to
any document created using the fonts or their derivatives.

 

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such.
This may include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components
as distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting ? in part or in whole ?
any of the components of the Original Version, by changing formats or
by porting the Font Software to a new environment.

"Author" refers to any designer, engineer, programmer, technical writer
or other person who contributed to the Font Software.


PERMISSION & CONDITIONS

Permission is hereby granted, free of charge, to any person obtaining a
copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,in
   Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
   redistributed and/or sold with any software, provided that each copy
   contains the above copyright notice and this license. These can be
   included either as stand-alone text files, human-readable headers or
   in the appropriate machine-readable metadata fields within text or
   binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
   Name(s) unless explicit written permission is granted by the
   corresponding Copyright Holder. This restriction only applies to the
   primary font name as presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
   Software shall not be used to promote, endorse or advertise any
   Modified Version, except to acknowledge the contribution(s) of the
   Copyright Holder(s) and the Author(s) or with their explicit written
   permission.

5) The Font Software, modified or unmodified, in part or in whole, must
   be distributed entirely under this license, and must not be distributed
   under any other license. The requirement for fonts to remain under
   this license does not apply to any document created using the Font
   Software.


 
TERMINATION
This license becomes null and void if any of the above conditions are not met.

 

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT.  IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM OTHER
DEALINGS IN THE FONT SOFTWARE.
```

</details>

## Third-party: the .pptx renderer's fonts (SIL Open Font License 1.1)

`k2-deckrender` draws slides with these fonts only (never the system's), compiled into the binary from
`crates/k2-deckrender/fonts/` (Liberation Sans: through the vendored deckcraft-fonts,
`vendor/deckcraft/crates/fonts/liberation-fonts-ttf-2.1.5/`). Every file is upstream's, byte for byte:
no subsetting, no WOFF2, no renaming, no table edits. `crates/k2-deckrender/fonts/manifest.tsv` lists
each file with its sha256, and `tests/fonts.rs` checks every file against it. Any other font is reviewed
for its licence before it ships. The OFL text is above (Liberation Sans) and beside each family as
shipped (`OFL.txt`, Liberation's `LICENSE`).

| Family | Copyright and Reserved Font Names | Licence files beside the fonts |
|---|---|---|
| Liberation Sans, Serif, Mono 2.1.5 | Digitized data copyright (c) 2010 Google Corporation, with Reserved Font Arimo, Tinos and Cousine. Copyright (c) 2012 Red Hat, Inc., with Reserved Font Name Liberation. | `LICENSE` (OFL-1.1), `AUTHORS`, `ChangeLog` |
| Carlito 1.104 | Copyright 2013 The Carlito Project Authors (https://github.com/googlefonts/carlito), with Reserved Font Name "Carlito". | `OFL.txt` |
| Caladea 1.001 | Copyright 2012 The Caladea Project Authors (https://github.com/huertatipografica/Caladea). No Reserved Font Name. | `OFL.txt`, `AUTHOR.txt`, `CONTRIBUTORS.txt` |
| Gelasio 1.008 | Copyright 2022 The Gelasio Project Authors (https://github.com/SorkinType/Gelasio). No Reserved Font Name. | `OFL.txt` |
| Roboto 3.015 | Copyright 2011 The Roboto Project Authors (https://github.com/googlefonts/roboto-classic). No Reserved Font Name. | `OFL.txt` |
| Poppins 4.004 | Copyright 2020 The Poppins Project Authors (https://github.com/itfoundry/Poppins). No Reserved Font Name. | `OFL.txt` |
| Noto Sans 2.015, Noto Serif 2.015, Noto Sans Mono 2.014, Noto Sans Symbols 2.003, Noto Sans Symbols 2 2.008, Noto Sans Math 3.000 | Copyright 2022 The Noto Project Authors (https://github.com/notofonts/latin-greek-cyrillic, …/symbols, …/math). No Reserved Font Name. | `OFL.txt` |

Every file (name, family and style, version, sha256, source):

| File | Family, style | Version | sha256 | Source |
|---|---|---|---|---|
| `LiberationSans-Regular.ttf` | Liberation Sans, Regular | 2.1.5 | `76d04c18ea243f426b7de1f3ad208e927008f961dc5945e5aad352d0dfde8ee8` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz |
| `LiberationSans-Bold.ttf` | Liberation Sans, Bold | 2.1.5 | `788abee4c806d660e8aee46689dd8540cd4bb98da03dcc9d171ce3efd99a9173` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz |
| `LiberationSans-Italic.ttf` | Liberation Sans, Italic | 2.1.5 | `e5bae5c4cde31f22142753855f4f8fb86da6ff39955ed3c0a11248b0d16948b0` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz |
| `LiberationSans-BoldItalic.ttf` | Liberation Sans, Bold Italic | 2.1.5 | `698da70fc191cc5f33ad4d6d3fe830fe4624b898ea2e3169955928b7c491f1ee` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz |
| `LiberationMono-Bold.ttf` | Liberation Mono, Bold | 2.1.5 | `bd62a0672d0b9b6710b01df434c80ad54fa5f0835207eb7b17b7a761463067bb` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz#liberation-fonts-ttf-2.1.5/LiberationMono-Bold.ttf |
| `LiberationMono-BoldItalic.ttf` | Liberation Mono, Bold Italic | 2.1.5 | `79451f3c09fe25116098853b7a2ca6e2436220ccc11af022979adbcf195be130` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz#liberation-fonts-ttf-2.1.5/LiberationMono-BoldItalic.ttf |
| `LiberationMono-Italic.ttf` | Liberation Mono, Italic | 2.1.5 | `605c01c711b44480a7508d349dfbf3264e81fa43d69e61cfa7d10b86e764c4d1` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz#liberation-fonts-ttf-2.1.5/LiberationMono-Italic.ttf |
| `LiberationMono-Regular.ttf` | Liberation Mono, Regular | 2.1.5 | `f2b83c763e8afd21709333370bed4774337fae82267937e2b5aea7e2fbd922c1` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz#liberation-fonts-ttf-2.1.5/LiberationMono-Regular.ttf |
| `LiberationSerif-Bold.ttf` | Liberation Serif, Bold | 2.1.5 | `d754ba427cfe0bca54ae052384baa8f842da5bd6550ad4da024ac441e7a7d5ce` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz#liberation-fonts-ttf-2.1.5/LiberationSerif-Bold.ttf |
| `LiberationSerif-BoldItalic.ttf` | Liberation Serif, Bold Italic | 2.1.5 | `f17db8af71e24d2066b587546021d4f0b296be389512b658dec3c09affeb11a7` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz#liberation-fonts-ttf-2.1.5/LiberationSerif-BoldItalic.ttf |
| `LiberationSerif-Italic.ttf` | Liberation Serif, Italic | 2.1.5 | `0e3dea9f8d613e006ccfa62201f33e265d19167bd0907725c3e145368b04fc2e` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz#liberation-fonts-ttf-2.1.5/LiberationSerif-Italic.ttf |
| `LiberationSerif-Regular.ttf` | Liberation Serif, Regular | 2.1.5 | `058ea80864aef09a23f45cbec2bb5400bc3dfbdea01c3f10538a21fcb497fb74` | https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz#liberation-fonts-ttf-2.1.5/LiberationSerif-Regular.ttf |
| `Carlito-Bold.ttf` | Carlito, Bold | 1.104 | `bb5d20f79b82599ec72983597437373a80f2d2085fa91fc144fd74e876a594db` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/carlito/Carlito-Bold.ttf |
| `Carlito-BoldItalic.ttf` | Carlito, Bold Italic | 1.104 | `b32928186c119599e03ca6a1ffc680fdcb7fac95772f4b95d989cf6cd3861517` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/carlito/Carlito-BoldItalic.ttf |
| `Carlito-Italic.ttf` | Carlito, Italic | 1.104 | `0b019225e58d702bfedcbd35c21696769f8ee115cb6343f84c2f240312450d1c` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/carlito/Carlito-Italic.ttf |
| `Carlito-Regular.ttf` | Carlito, Regular | 1.104 | `f6418f708baede9789daef5d458c0f53d2a888af9820e8062934e504fedc6595` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/carlito/Carlito-Regular.ttf |
| `Caladea-Bold.ttf` | Caladea, Bold | 1.001 | `ae3cb2dcbc925809dd29d2a44e9802211cab66be541bacbfc9c08c74b27c3742` | https://raw.githubusercontent.com/huertatipografica/Caladea/336a529cfad3d103d6527752686f8331d13e820a/fonts/ttf/Caladea-Bold.ttf |
| `Caladea-BoldItalic.ttf` | Caladea, Bold Italic | 1.001 | `ccabaa7b7e2fdf253d2b1a5fa699dd8a3df8d835a9eb285ad82631a677eb76c0` | https://raw.githubusercontent.com/huertatipografica/Caladea/336a529cfad3d103d6527752686f8331d13e820a/fonts/ttf/Caladea-BoldItalic.ttf |
| `Caladea-Italic.ttf` | Caladea, Italic | 1.001 | `4359a8e24f748b6447b1ff6d7a174febe70961d29f8bb8634b56dacd740a3deb` | https://raw.githubusercontent.com/huertatipografica/Caladea/336a529cfad3d103d6527752686f8331d13e820a/fonts/ttf/Caladea-Italic.ttf |
| `Caladea-Regular.ttf` | Caladea, Regular | 1.001 | `f1e899278b7b4491aba5b6a8253c4b04c050cc59b21865be5c37559a775153cd` | https://raw.githubusercontent.com/huertatipografica/Caladea/336a529cfad3d103d6527752686f8331d13e820a/fonts/ttf/Caladea-Regular.ttf |
| `Gelasio-Italic[wght].ttf` | Gelasio, Italic | 1.008 | `52559e845a4d33514e5f93bb9ae7dbeae1894a53f2c565a15f18af40cd337c09` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/gelasio/Gelasio-Italic%5Bwght%5D.ttf |
| `Gelasio[wght].ttf` | Gelasio, Regular | 1.008 | `4daecea457258c9ebeb8bc99ed3fd24353618bfad3ea4b93fa0b5d0468fc04e4` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/gelasio/Gelasio%5Bwght%5D.ttf |
| `Roboto-Italic[wdth,wght].ttf` | Roboto, Italic | 3.015 | `9725a847af6b460ffca162ae66d20dad48b01876137947180b42d7dcd7887182` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/roboto/Roboto-Italic%5Bwdth,wght%5D.ttf |
| `Roboto[wdth,wght].ttf` | Roboto, Regular | 3.015 | `d7598e12c5dbef095ff8272cfc55da0250bd07fbdecbac8a530b9b277872a134` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/roboto/Roboto%5Bwdth,wght%5D.ttf |
| `Poppins-Black.ttf` | Poppins, Black | 4.004 | `d82aaaf98a9283f9a8edd24e51173337d8eaf09e25cd3d98831f8ec8461748a1` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-Black.ttf |
| `Poppins-BlackItalic.ttf` | Poppins, Black Italic | 4.004 | `f4852ca89c29f69d800e14f097ed4d1f0a0cc454e9f77d73bfd6db1f71c287a0` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-BlackItalic.ttf |
| `Poppins-Bold.ttf` | Poppins, Bold | 4.004 | `983676516167748b74de6f4771fb384c664fd913acb8b471122ecacf5da5ea6c` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-Bold.ttf |
| `Poppins-BoldItalic.ttf` | Poppins, Bold Italic | 4.004 | `3572ac8116a0ac7317d342262b29937bcbaf94d8f03f90df6fe666fa7e2fb43a` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-BoldItalic.ttf |
| `Poppins-ExtraBold.ttf` | Poppins, ExtraBold | 4.004 | `f2ab17c1a63a0ecc12c2461848fc8a469395e3cd2d641803e889c643d9f958e1` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-ExtraBold.ttf |
| `Poppins-ExtraBoldItalic.ttf` | Poppins, ExtraBold Italic | 4.004 | `dc00a2eb988373e9f4e99bf8ff76c6315ee21d36b341cbbff024e4c18cc7ac03` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-ExtraBoldItalic.ttf |
| `Poppins-ExtraLight.ttf` | Poppins, ExtraLight | 4.004 | `55c03314cc754e26f741f97890e5e9cbe3b3278fe3abcecfecafd60111b2643d` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-ExtraLight.ttf |
| `Poppins-ExtraLightItalic.ttf` | Poppins, ExtraLight Italic | 4.004 | `7171a17e4cda8f6ded78a7a931f9b7f38987057f3ffd1fe07a71528ad0f01e49` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-ExtraLightItalic.ttf |
| `Poppins-Italic.ttf` | Poppins, Italic | 4.004 | `4fa76ae75b40f926420514044722cb97f32186cafd3b38263cc34dad7174d46d` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-Italic.ttf |
| `Poppins-Light.ttf` | Poppins, Light | 4.004 | `650ba57fa99d12ec40c31ccfb680be656be4497fbe14164617d67e32ffe9cd46` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-Light.ttf |
| `Poppins-LightItalic.ttf` | Poppins, Light Italic | 4.004 | `b8f9c5be59723fadf8e5447fa1245c2c53b60a3464a24d6ece9ee3c283d8917b` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-LightItalic.ttf |
| `Poppins-Medium.ttf` | Poppins, Medium | 4.004 | `90373e7d838d32468438fc3e152dca0bdb12edcab99ea639f158790b1ba1fd05` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-Medium.ttf |
| `Poppins-MediumItalic.ttf` | Poppins, Medium Italic | 4.004 | `765addf6c7c11ec3c54325cacd68cabd05df8e4b6455302d812a9b9bafd1c614` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-MediumItalic.ttf |
| `Poppins-Regular.ttf` | Poppins, Regular | 4.004 | `7e65201e9b79159e2300267cc885e16c8dcef2424cdfa09a29bfb0980a94a7ba` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-Regular.ttf |
| `Poppins-SemiBold.ttf` | Poppins, SemiBold | 4.004 | `d3bf1bdaf0550e83da9ac0b1d1d9fe6db086835a83aa28578e609a394b9a0286` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-SemiBold.ttf |
| `Poppins-SemiBoldItalic.ttf` | Poppins, SemiBold Italic | 4.004 | `16bb118aa232c9a13fa238027d24d7854dd1a1d9cbaf99b17fec4388d56b432c` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-SemiBoldItalic.ttf |
| `Poppins-Thin.ttf` | Poppins, Thin | 4.004 | `6d8e5d9d29140cc93e321745fa1243c67889e6bc3639ec34db64f3da7a496352` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-Thin.ttf |
| `Poppins-ThinItalic.ttf` | Poppins, Thin Italic | 4.004 | `46df80ac970f5e84829b868d283878b4d97e289c7b1245541d8bbf66b5d670a8` | https://raw.githubusercontent.com/google/fonts/86aced91de05cd4f96a06838124cca45be337b2b/ofl/poppins/Poppins-ThinItalic.ttf |
| `NotoSans-Italic[wdth,wght].ttf` | Noto Sans, Italic | 2.015 | `0c9c7620381d710ac16269b1f1756f9ee05fa4c3bf7293ff04ef6723fcddbe9f` | https://github.com/notofonts/latin-greek-cyrillic/releases/download/NotoSans-v2.015/NotoSans-v2.015.zip#NotoSans/unhinted/variable-ttf/NotoSans-Italic[wdth,wght].ttf |
| `NotoSans[wdth,wght].ttf` | Noto Sans, Regular | 2.015 | `511cc04b442ffcc3b24d65c2cc780bdffaf482b20ba0aebdc8f27b37017ff0f5` | https://github.com/notofonts/latin-greek-cyrillic/releases/download/NotoSans-v2.015/NotoSans-v2.015.zip#NotoSans/unhinted/variable-ttf/NotoSans[wdth,wght].ttf |
| `NotoSerif-Italic[wdth,wght].ttf` | Noto Serif, Italic | 2.015 | `e5c3ade5b59b3af8c48c24915580cab99f083bd728a81c343741fc09a0c76c9f` | https://github.com/notofonts/latin-greek-cyrillic/releases/download/NotoSerif-v2.015/NotoSerif-v2.015.zip#NotoSerif/unhinted/variable-ttf/NotoSerif-Italic[wdth,wght].ttf |
| `NotoSerif[wdth,wght].ttf` | Noto Serif, Regular | 2.015 | `8918809882ffe61eb32baf3f4b3420fda6c022f2219b1b0272755d70ecabc7de` | https://github.com/notofonts/latin-greek-cyrillic/releases/download/NotoSerif-v2.015/NotoSerif-v2.015.zip#NotoSerif/unhinted/variable-ttf/NotoSerif[wdth,wght].ttf |
| `NotoSansMono-Bold.ttf` | Noto Sans Mono, Bold | 2.014 | `1100772b2f79c102402a1011df5e2226517d0f40ac553a25fb12fdbad8c11b85` | https://github.com/notofonts/latin-greek-cyrillic/releases/download/NotoSansMono-v2.014/NotoSansMono-v2.014.zip#NotoSansMono/unhinted/ttf/NotoSansMono-Bold.ttf |
| `NotoSansMono-Regular.ttf` | Noto Sans Mono, Regular | 2.014 | `87f8ce0522a6c99b743ee5fc75b4073cfdd575639119672828b7b9944b65b4f4` | https://github.com/notofonts/latin-greek-cyrillic/releases/download/NotoSansMono-v2.014/NotoSansMono-v2.014.zip#NotoSansMono/unhinted/ttf/NotoSansMono-Regular.ttf |
| `NotoSansSymbols[wght].ttf` | Noto Sans Symbols, Regular | 2.003 | `9dc0ef3c6eea7436e064409ae9d0e3cc1928e21ec5c1baf43096cfad208b2d0a` | https://github.com/notofonts/symbols/releases/download/NotoSansSymbols-v2.003/NotoSansSymbols-v2.003.zip#NotoSansSymbols/unhinted/variable-ttf/NotoSansSymbols[wght].ttf |
| `NotoSansSymbols2-Regular.ttf` | Noto Sans Symbols 2, Regular | 2.008 | `c4a0a80f0041ce4be81e2478faad22776d23edb98ae3f0d19bd37044820ecf9d` | https://github.com/notofonts/symbols/releases/download/NotoSansSymbols2-v2.008/NotoSansSymbols2-v2.008.zip#NotoSansSymbols2/unhinted/ttf/NotoSansSymbols2-Regular.ttf |
| `NotoSansMath-Regular.ttf` | Noto Sans Math, Regular | 3.000 | `b127e84699212b6b2ef50aff58e0ebebeec04ffe6db1b9eb9e209c8c3d97b4aa` | https://github.com/notofonts/math/releases/download/NotoSansMath-v3.000/NotoSansMath-v3.000.zip#NotoSansMath/unhinted/ttf/NotoSansMath-Regular.ttf |

Sources: Liberation from the liberation-fonts 2.1.5 release tarball (sha256 `7191c669bf38899f73a2094ed00f7b800553364f90e2637010a69c0e268f25d0`); Carlito, Gelasio, Roboto and Poppins from google/fonts at commit `86aced91de05cd4f96a06838124cca45be337b2b` (`ofl/`); Caladea from huertatipografica/Caladea at commit `336a529cfad3d103d6527752686f8331d13e820a` (`fonts/ttf/`); Noto from the notofonts release zips named in each row (their `unhinted` builds). Zip sha256: NotoSans-v2.015 `0c34df072a3fa7efbb7cbf34950e1f971a4447cffe365d3a359e2d4089b958f5`, NotoSerif-v2.015 `0e9a43c8a4b94ac76f55069ed1d7385bbcaf6b99527a94deb5619e032b7e76c1`, NotoSansMono-v2.014 `090cf6c5e03f337a755630ca888b1fef463e64ae7b33ee134e9309c05f978732`, NotoSansSymbols-v2.003 `0c113cdcf6c31d050b80dac39fba2d804a6985281012e76e9220c0a00da007f3`, NotoSansSymbols2-v2.008 `346c930bbe8eb946701a05c54e9c11a2094dee1d93c387bf1771c0a3e335688f`, NotoSansMath-v3.000 `ac351837b41f8a897f020b97fb0f075ad574c1e9669fb5839ada1f92fd748356`.
K2's code is **MIT**. The font files keep their own licence and do **not** relicense the product.

## Third-party: Fira Code 6.2 (SIL Open Font License 1.1)

One of the desktop app's terminal and editor font choices, bundled into the app's renderer and
loaded by `@font-face` in `src/renderer/globals.css`: two styles of Fira Code **6.2**, unmodified
and whole.

| | |
|--|--|
| **What** | `src/renderer/fonts/FiraCode-Regular.ttf` (sha256 `5992ab9640e2df491b2f609467b1de60e8bc39b2c28db184342a0592d98f6117`) and `src/renderer/fonts/FiraCode-Bold.ttf` (sha256 `41f6554e845e2f5b70adad3950122334b866aac436793b7742ade600067701be`); font version `6.002` |
| **Upstream** | [tonsky/FiraCode](https://github.com/tonsky/FiraCode), release 6.2 (`Fira_Code_v6.2.zip`, sha256 `0949915ba8eb24d89fd93d10a7ff623f42830d7c5ffc3ecbf960e4ecad3e3e79`); both files are byte for byte the zip's `ttf/FiraCode-Regular.ttf` and `ttf/FiraCode-Bold.ttf` |
| **Copyright** | Copyright (c) 2014, The Fira Code Project Authors (https://github.com/tonsky/FiraCode) (upstream's `LICENSE` at tag 6.2; the fonts' own name table says "Copyright 2014-2021 The Fira Code Project Authors (https://github.com/tonsky/FiraCode)"). No Reserved Font Name. The name table also says "Fira Mono is a trademark of The Mozilla Corporation." |
| **License** | **SIL Open Font License, Version 1.1**, full text below as upstream ships it. The release zip carries no licence file, so the text is the repository's `LICENSE` at tag `6.2` (commit `eee6db993696aba61ff4eef03698e2987d79910c`). The fonts are not renamed, subset or modified |
| **K2 code** | Is **MIT**. The font files keep their own licence and do **not** relicense the product. |

<details>
<summary>SIL Open Font License, Version 1.1 (Fira Code: LICENSE, tonsky/FiraCode tag 6.2, commit eee6db993696aba61ff4eef03698e2987d79910c)</summary>

```
Copyright (c) 2014, The Fira Code Project Authors (https://github.com/tonsky/FiraCode)

This Font Software is licensed under the SIL Open Font License, Version 1.1.
This license is copied below, and is also available with a FAQ at:
http://scripts.sil.org/OFL


-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

</details>

## Third-party: JetBrains Mono 2.304 (SIL Open Font License 1.1)

One of the desktop app's terminal and editor font choices, bundled into the app's renderer and
loaded by `@font-face` in `src/renderer/globals.css`: the Regular style of JetBrains Mono
**2.304**, unmodified and whole.

| | |
|--|--|
| **What** | `src/renderer/fonts/JetBrainsMono-Regular.ttf` (sha256 `a0bf60ef0f83c5ed4d7a75d45838548b1f6873372dfac88f71804491898d138f`); font version `2.304; ttfautohint (v1.8.4.7-5d5b)` |
| **Upstream** | [JetBrains/JetBrainsMono](https://github.com/JetBrains/JetBrainsMono), release v2.304 (`JetBrainsMono-2.304.zip`, sha256 `6f6376c6ed2960ea8a963cd7387ec9d76e3f629125bc33d1fdcd7eb7012f7bbf`); the file is byte for byte the zip's `fonts/ttf/JetBrainsMono-Regular.ttf` |
| **Copyright** | Copyright 2020 The JetBrains Mono Project Authors (https://github.com/JetBrains/JetBrainsMono). No Reserved Font Name. The name table also says "JetBrains Mono is a trademark of JetBrains s.r.o." |
| **License** | **SIL Open Font License, Version 1.1**, full text below as the release zip's `OFL.txt` ships it (the same bytes as the repository's `OFL.txt` at tag `v2.304`, commit `cd5227bd1f61dff3bbd6c814ceaf7ffd95e947d9`). The font is not renamed, subset or modified |
| **K2 code** | Is **MIT**. The font file keeps its own licence and does **not** relicense the product. |

<details>
<summary>SIL Open Font License, Version 1.1 (JetBrains Mono: OFL.txt from the release zip)</summary>

```
Copyright 2020 The JetBrains Mono Project Authors (https://github.com/JetBrains/JetBrainsMono)

This Font Software is licensed under the SIL Open Font License, Version 1.1.
This license is copied below, and is also available with a FAQ at:
https://scripts.sil.org/OFL


-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded, 
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

</details>

## Third-party: Lilex 2.601 (SIL Open Font License 1.1)

One of the desktop app's terminal and editor font choices, bundled into the app's renderer and
loaded by `@font-face` in `src/renderer/globals.css`: three styles of Lilex **2.601**, unmodified
and whole.

| | |
|--|--|
| **What** | `src/renderer/fonts/Lilex-Regular.ttf` (sha256 `8a467ad5a1941dc7c9cebaee34e3ed9364a73b48393034e363e4b528bd711d04`), `src/renderer/fonts/Lilex-Bold.ttf` (sha256 `3369214e2ecddb45ae5024b11d45dd637e99e9d2127ab729817d41d72b8a9c68`) and `src/renderer/fonts/Lilex-Italic.ttf` (sha256 `a6e52046b91ed12a6af50bb74e70e27dea4a54eab2bed8947db4c1128ba83d45`); font version `2.601; ttfautohint (v1.8.4.7-5d5b)` |
| **Upstream** | [mishamyrt/Lilex](https://github.com/mishamyrt/Lilex), release 2.601 (`Lilex.zip`, sha256 `aa581a14dc581317ab2cfb43098256d7ba61f7b11670084808de859e97e58e58`, as GitHub lists for the asset); the three files are byte for byte the zip's `ttf/Lilex-Regular.ttf`, `ttf/Lilex-Bold.ttf` and `ttf/Lilex-Italic.ttf` |
| **Copyright** | Copyright 2019 The Lilex Project Authors (https://github.com/mishamyrt/Lilex). No Reserved Font Name. |
| **License** | **SIL Open Font License, Version 1.1**, full text below as upstream ships it. The release zip carries no licence file, so the text is the repository's `OFL.txt` at tag `2.601` (commit `506b1a32971d2a2c2bdc82149e4578b4da4b8cc4`). The fonts are not renamed, subset or modified |
| **K2 code** | Is **MIT**. The font files keep their own licence and do **not** relicense the product. |

<details>
<summary>SIL Open Font License, Version 1.1 (Lilex: OFL.txt, mishamyrt/Lilex tag 2.601, commit 506b1a32971d2a2c2bdc82149e4578b4da4b8cc4)</summary>

```
Copyright 2019 The Lilex Project Authors (https://github.com/mishamyrt/Lilex)

This Font Software is licensed under the SIL Open Font License, Version 1.1.
This license is copied below, and is also available with a FAQ at:
https://scripts.sil.org/OFL


-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded, 
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

</details>

## Third-party: MesloLGM Nerd Font Mono, Nerd Fonts 3.4.0 (Apache-2.0, MIT, and each glyph set's own licence)

The desktop app's default terminal and interface font, and the font the daemon measures terminal
cells with: three styles of MesloLGM Nerd Font Mono from the Nerd Fonts **v3.4.0** release,
unmodified and whole.

These glyph sets keep their own licences, listed below.

| | |
|--|--|
| **What** | `src/renderer/fonts/MesloLGMNerdFontMono-Regular.ttf`, `-Bold.ttf` and `-Italic.ttf`, bundled into the desktop app's renderer and loaded by `@font-face` in `src/renderer/globals.css`. The same three files, byte for byte, are in `crates/k2-core/fonts/`; the daemon compiles in the Regular (`include_bytes!` in `crates/k2-core/src/terminal/font_renderer.rs`), and the Bold and Italic there are not compiled in. Font version `1.210;Nerd Fonts 3.4.0` |
| **Upstream** | [ryanoasis/nerd-fonts](https://github.com/ryanoasis/nerd-fonts), release v3.4.0 (`Meslo.zip`, sha256 `13b502ac8c2bd9d3161018064560e23cd42b175bb730780a270975265a19ad57`, matching the release's `SHA-256.txt`); each file is byte for byte the zip's file of the same name |
| **Files** | `MesloLGMNerdFontMono-Regular.ttf` sha256 `a440f036bcb21a13b9c0b6852d2e53a5df4752d5be4de318046fab4c52d7ce55`; `MesloLGMNerdFontMono-Bold.ttf` sha256 `c76da4d9fca24be442437f4f7cbae99acad21971f81543588c7f9da14e8ce8ec`; `MesloLGMNerdFontMono-Italic.ttf` sha256 `9791f8376801ec8806df7b5ae8af07fea7c567a9a4bb0a33d35e3b415cffe13e` |
| **Meslo LG** | **Apache-2.0**, Copyright 2009, 2010, 2013 André Berg, [andreberg/Meslo-Font](https://github.com/andreberg/Meslo-Font); the notice is the release zip's `LICENSE.txt`, below with the full Apache License 2.0. Meslo LG is a customized version of Apple's Menlo-Regular (itself a customized Bitstream Vera Sans Mono); the fonts' name table says "Copyright © 2009 Apple Inc. Copyright ©  2006 by Tavmjong Bah. Copyright © 2003 by Bitstream, Inc. All Rights Reserved." and "Menlo is a Trademark of Apple Inc." Meslo's README (`README.textile` on andreberg/Meslo-Font `master`, commit `09a431d546d211130352c28eb0466e5d7d5aeaf0`) adds "Bitstream Vera is a trademark of Bitstream, Inc., designed by Jim Lyles." |
| **Nerd Fonts** | The patcher and the project's own source are **MIT**, Copyright (c) 2014 Ryan L McIntyre; the project's own glyph fonts are under the SIL OFL 1.1, Copyright (c) 2014, Ryan L McIntyre. Both texts are in upstream's `LICENSE` (Nerd Fonts tag `v3.4.0`, commit `fa7b859994228a9c8759f99c55a8d31ee92a1b5e`), below in full. That `LICENSE` also says patched fonts are under the OFL; its `license-audit.md` lists Meslo LG as Apache-2.0, and the release zip ships Meslo's Apache-2.0 `LICENSE.txt` beside these files, so both texts are given here |
| **Added glyphs** | Each set's own licence, as listed in upstream's `license-audit.md` and the licence files under `src/glyphs/` (both at Nerd Fonts tag `v3.4.0`, commit `fa7b859994228a9c8759f99c55a8d31ee92a1b5e`); see the table below |
| **Reserved Font Names** | "Pomicons" and "Font Awesome" (from their licences). K2 uses neither name for a font. |
| **Changes** | None by K2. The glyph sets were adapted by the Nerd Fonts project when it patched them in (scaled and placed into Meslo; Codicons glyphs 0xEB40 and 0xEB41 fixed; Font Awesome rebuilt from the 6.5.1 release SVGs, not every icon) |
| **K2 code** | Is **MIT**. The font files keep their own licences and do **not** relicense the product. |

Our reading: The Meslo font program is under Meslo LG's Apache-2.0, as Nerd Fonts' license-audit.md and the release zip's LICENSE.txt give it. Nerd Fonts' own patcher work is MIT. The OFL text applies to the OFL glyph sets.

The glyph sets in these files (all present in all three styles):

| Set | Version in Nerd Fonts 3.4.0 | Copyright | Licence |
|---|---|---|---|
| Codicons | 0.0.35 | Microsoft Corporation, [microsoft/vscode-codicons](https://github.com/microsoft/vscode-codicons) (no copyright notice ships with the icons) | **CC BY 4.0**, https://creativecommons.org/licenses/by/4.0/ (attribution below) |
| Font Awesome | 6.5.1 (Nerd Fonts' own build from the release SVGs) | Copyright (c) 2023 Fonticons, Inc. (https://fontawesome.com), as Font Awesome's `LICENSE.txt` at tag `6.5.1` has it (Nerd Fonts' copy, below, says 2022) | Icons: **CC BY 4.0**, https://creativecommons.org/licenses/by/4.0/ (attribution below). Upstream's licence file, below in full, also gives the SIL OFL 1.1 for its font files, with Reserved Font Name "Font Awesome" |
| Font Logos | 1.3.0 | Copyright (c) 2014-2024, Lukas W (font name table), [lukas-w/font-logos](https://github.com/lukas-w/font-logos) | **The Unlicense** (upstream's `LICENSE` at tag `v1.3.0`, commit `e92426e28ee5bbf4ca328b33e3a393b7a5045f01`, below; Nerd Fonts' audit writes "Unlicensed"). "All brand icons are trademarks of their respective owners and should only be used to represent the company or product to which they refer." |
| Material Design Icons | as fetched by Nerd Fonts (October 2022) | Pictogrammers, [Templarian/MaterialDesign-Font](https://github.com/Templarian/MaterialDesign-Font) | **Apache-2.0** (Pictogrammers Free License, below) |
| Octicons | as in Nerd Fonts 3.4.0 | Copyright (c) 2023 GitHub Inc. | **MIT** (below) |
| Devicons | 2.16.0 (Nerd Fonts' own build) | Copyright (c) the Devicons contributors, https://github.com/devicons/devicon | **MIT** (below) |
| Font Awesome Extension | as in Nerd Fonts 3.4.0 | Copyright (c) the Font Awesome Extension contributors, https://github.com/AndreLZGava/font-awesome-extension | **MIT** (below) |
| IEC Power Symbols | as in Nerd Fonts 3.4.0 | Copyright (c) the IEC Power Symbols contributors, https://unicodepowersymbol.com/ | **MIT** (below) |
| Seti-UI and Original (Nerd Fonts' own) | as in Nerd Fonts 3.4.0 | Copyright (c) 2014 Ryan L McIntyre; the Seti-UI glyphs are Jesse Weed's (MIT, see the Seti section above), modified | **MIT** (Nerd Fonts' `LICENSE`, below) |
| Powerline Extra Symbols | as in Nerd Fonts 3.4.0 | Copyright (c) 2016 Ryan L McIntyre | **MIT** (below) |
| Powerline Symbols | as in Nerd Fonts 3.4.0 | Copyright 2013 Kim Silkebækken and other contributors, https://github.com/powerline/powerline | Its own permissive licence, MIT wording (below; Nerd Fonts' audit writes "Free License") |
| Pomicons | 1.001 | Copyright (c) 2021, Gabriele Lana, with Reserved Font Name Pomicons | **SIL OFL 1.1** (below). The Pomicons font's own name table says "Copyright (c) 2014 by Davide Bignotti." |
| Weather Icons | 1.100 (font name table) | Erik Flowers, with the original icon designs by Lukas Bischoff, [erikflowers/weather-icons](https://github.com/erikflowers/weather-icons). The holder is taken from the credit line in upstream's README ("The icon designs are originally by Lukas Bischoff. Icon art for v1.1 forward, HTML, Less, and CSS are by me (Erik).", `master`, commit `bb80982bf1f43f2d57f9dd753e7413bf88beb9ed`), not from the `OFL.txt` Nerd Fonts ships, whose copyright lines are the unfilled OFL template | **SIL OFL 1.1** (below) |

The Devicons, Font Awesome Extension and IEC Power Symbols holders come from each project, not from a licence file: Nerd Fonts v3.4.0 ships no licence file for these sets, and its `license-audit.md` gives them as MIT.

**Attribution for the CC BY 4.0 glyphs.** Neither creator endorses K2, and the licence grants no
rights to their names, logos or trademarks.

- **Codicons** 0.0.35. Creator: Microsoft Corporation. Copyright notice: none ships with the icons
  (the repository's licence for its code reads "Copyright (c) Microsoft Corporation."). Licence:
  Creative Commons Attribution 4.0 International (CC BY 4.0),
  https://creativecommons.org/licenses/by/4.0/. Material:
  https://github.com/microsoft/vscode-codicons/tree/0.0.35 (commit
  `080608815546b776a6397e9566d47b5996a78546`). As adapted into the font by Nerd Fonts; not modified
  further by Alakazam Labs.
- **Font Awesome Free** 6.5.1 icons. Creator: Fonticons, Inc. (https://fontawesome.com). Copyright
  notice: "Copyright (c) 2023 Fonticons, Inc. (https://fontawesome.com)" (Font Awesome's
  `LICENSE.txt` at tag `6.5.1`; the copy Nerd Fonts ships says 2022). Licence: Creative Commons
  Attribution 4.0 International (CC BY 4.0), https://creativecommons.org/licenses/by/4.0/.
  Material: https://github.com/FortAwesome/Font-Awesome/tree/6.5.1 (commit
  `deeea78c52bfe00b6e251ffddccf5570d5fdb05e`). As adapted into the font by Nerd Fonts; not modified
  further by Alakazam Labs.

<details>
<summary>Meslo LG notice (Apache-2.0), as the Nerd Fonts release ships it</summary>

```
Copyright 2009, 2010, 2013 André Berg

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```

</details>

<details>
<summary>Apache License, Version 2.0 (from https://www.apache.org/licenses/LICENSE-2.0.txt)</summary>

```
                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

   1. Definitions.

      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.

      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.

      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.

      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.

      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.

      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.

      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).

      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.

      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."

      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by Licensor and
      subsequently incorporated within the Work.

   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.

   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.

   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:

      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and

      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and

      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and

      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding those notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.

      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.

   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.

   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.

   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.

   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or consequential damages of any character arising as a
      result of this License or out of the use or inability to use the
      Work (including but not limited to damages for loss of goodwill,
      work stoppage, computer failure or malfunction, or any and all
      other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.

   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on Your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.

   END OF TERMS AND CONDITIONS

   APPENDIX: How to apply the Apache License to your work.

      To apply the Apache License to your work, attach the following
      boilerplate notice, with the fields enclosed by brackets "[]"
      replaced with your own identifying information. (Don't include
      the brackets!)  The text should be enclosed in the appropriate
      comment syntax for the file format. We also recommend that a
      file or class name and description of purpose be included on the
      same "printed page" as the copyright notice for easier
      identification within third-party archives.

   Copyright [yyyy] [name of copyright owner]

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.
```

</details>

<details>
<summary>Nerd Fonts LICENSE (MIT and SIL OFL 1.1), Nerd Fonts tag v3.4.0, commit fa7b859994228a9c8759f99c55a8d31ee92a1b5e</summary>

```
# Nerd Fonts Licensing

There are various sources used under various licenses:

* Nerd Fonts source fonts, patched fonts, and folders with explict OFL SIL files are licensed under SIL OPEN FONT LICENSE Version 1.1 (see below).
* Nerd Fonts original source code files (such as `.sh`, `.py`, `font-patcher` and others) are licensed under the MIT License (MIT) (see below).
* Many other licenses are present in this project for even more detailed breakdown see: [License Audit](https://github.com/ryanoasis/nerd-fonts/blob/-/license-audit.md).

## Source files not in folders containing an explicit license are using the MIT License (MIT)

The MIT License (MIT)

Copyright (c) 2014 Ryan L McIntyre

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Various Fonts, Patched Fonts, SVGs, Glyph Fonts, and any files in a folder with explicit SIL OFL 1.1 License

Copyright (c) 2014, Ryan L McIntyre (https://ryanlmcintyre.com).

This Font Software is licensed under the SIL Open Font License, Version 1.1.
This license is copied below, and is also available with a FAQ at:
http://scripts.sil.org/OFL

-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded, 
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

</details>

<details>
<summary>Font Awesome Free licence (src/glyphs/font-awesome/LICENSE.txt, Nerd Fonts tag v3.4.0, commit fa7b859994228a9c8759f99c55a8d31ee92a1b5e)</summary>

```
Fonticons, Inc. (https://fontawesome.com)

--------------------------------------------------------------------------------

Font Awesome Free License

Font Awesome Free is free, open source, and GPL friendly. You can use it for
commercial projects, open source projects, or really almost whatever you want.
Full Font Awesome Free license: https://fontawesome.com/license/free.

--------------------------------------------------------------------------------

# Icons: CC BY 4.0 License (https://creativecommons.org/licenses/by/4.0/)

The Font Awesome Free download is licensed under a Creative Commons
Attribution 4.0 International License and applies to all icons packaged
as SVG and JS file types.

--------------------------------------------------------------------------------

# Fonts: SIL OFL 1.1 License

In the Font Awesome Free download, the SIL OFL license applies to all icons
packaged as web and desktop font files.

Copyright (c) 2022 Fonticons, Inc. (https://fontawesome.com)
with Reserved Font Name: "Font Awesome".

This Font Software is licensed under the SIL Open Font License, Version 1.1.
This license is copied below, and is also available with a FAQ at:
http://scripts.sil.org/OFL

SIL OPEN FONT LICENSE
Version 1.1 - 26 February 2007

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting — in part or in whole — any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.

--------------------------------------------------------------------------------

# Code: MIT License (https://opensource.org/licenses/MIT)

In the Font Awesome Free download, the MIT license applies to all non-font and
non-icon files.

Copyright 2022 Fonticons, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in the
Software without restriction, including without limitation the rights to use, copy,
modify, merge, publish, distribute, sublicense, and/or sell copies of the Software,
and to permit persons to whom the Software is furnished to do so, subject to the
following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT
HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

--------------------------------------------------------------------------------

# Attribution

Attribution is required by MIT, SIL OFL, and CC BY licenses. Downloaded Font
Awesome Free files already contain embedded comments with sufficient
attribution, so you shouldn't need to do anything additional when using these
files normally.

We've kept attribution comments terse, so we ask that you do not actively work
to remove them from files, especially code. They're a great way for folks to
learn about Font Awesome.

--------------------------------------------------------------------------------

# Brand Icons

All brand icons are trademarks of their respective owners. The use of these
trademarks does not indicate endorsement of the trademark holder by Font
Awesome, nor vice versa. **Please do not use brand logos for any purpose except
to represent the company, product, or service to which they refer.**
```

</details>

<details>
<summary>Font Logos: the Unlicense (LICENSE, lukas-w/font-logos tag v1.3.0, commit e92426e28ee5bbf4ca328b33e3a393b7a5045f01)</summary>

```
This is free and unencumbered software released into the public domain.

Anyone is free to copy, modify, publish, use, compile, sell, or
distribute this software, either in source code form or as a compiled
binary, for any purpose, commercial or non-commercial, and by any
means.

In jurisdictions that recognize copyright laws, the author or authors
of this software dedicate any and all copyright interest in the
software to the public domain. We make this dedication for the benefit
of the public at large and to the detriment of our heirs and
successors. We intend this dedication to be an overt act of
relinquishment in perpetuity of all present and future rights to this
software under copyright law.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR ANY CLAIM, DAMAGES OR
OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
OTHER DEALINGS IN THE SOFTWARE.

For more information, please refer to <http://unlicense.org>
```

</details>

<details>
<summary>Material Design Icons: Pictogrammers Free License (src/glyphs/materialdesign/LICENSE, Nerd Fonts tag v3.4.0, commit fa7b859994228a9c8759f99c55a8d31ee92a1b5e)</summary>

```
Pictogrammers Free License
--------------------------

This icon collection is released as free, open source, and GPL friendly by
the [Pictogrammers](http://pictogrammers.com/) icon group. You may use it
for commercial projects, open source projects, or anything really.

# Icons: Apache 2.0 (https://www.apache.org/licenses/LICENSE-2.0)
Some of the icons are redistributed under the Apache 2.0 license. All other
icons are either redistributed under their respective licenses or are
distributed under the Apache 2.0 license.

# Fonts: Apache 2.0 (https://www.apache.org/licenses/LICENSE-2.0)
All web and desktop fonts are distributed under the Apache 2.0 license. Web
and desktop fonts contain some icons that are redistributed under the Apache
2.0 license. All other icons are either redistributed under their respective
licenses or are distributed under the Apache 2.0 license.

# Code: MIT (https://opensource.org/licenses/MIT)
The MIT license applies to all non-font and non-icon files.
```

</details>

<details>
<summary>Octicons (MIT, src/glyphs/octicons/LICENSE, Nerd Fonts tag v3.4.0, commit fa7b859994228a9c8759f99c55a8d31ee92a1b5e)</summary>

```
MIT License

Copyright (c) 2023 GitHub Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

</details>

<details>
<summary>Powerline Extra Symbols (MIT, src/glyphs/powerline-extra/LICENSE, Nerd Fonts tag v3.4.0, commit fa7b859994228a9c8759f99c55a8d31ee92a1b5e)</summary>

```
MIT License

Copyright (c) 2016 Ryan L McIntyre

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

</details>

<details>
<summary>Powerline Symbols (src/glyphs/powerline-symbols/LICENSE.txt, Nerd Fonts tag v3.4.0, commit fa7b859994228a9c8759f99c55a8d31ee92a1b5e)</summary>

```
Copyright 2013 Kim Silkebækken and other contributors
https://github.com/powerline/powerline

Permission is hereby granted, free of charge, to any person obtaining 
a copy of this software and associated documentation files (the 
"Software"), to deal in the Software without restriction, including 
without limitation the rights to use, copy, modify, merge, publish, 
distribute, sublicense, and/or sell copies of the Software, and to 
permit persons to whom the Software is furnished to do so, subject to 
the following conditions:

The above copyright notice and this permission notice shall be 
included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, 
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF 
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  
IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY 
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, 
TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE 
SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

</details>

<details>
<summary>Devicons, Font Awesome Extension and IEC Power Symbols (MIT)</summary>

The holders below come from each project, not from a licence file (Nerd Fonts v3.4.0 ships none for these sets).

```
MIT License

Copyright (c) the Devicons contributors (https://github.com/devicons/devicon)
Copyright (c) the Font Awesome Extension contributors (https://github.com/AndreLZGava/font-awesome-extension)
Copyright (c) the IEC Power Symbols contributors (https://unicodepowersymbol.com/)

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

</details>

<details>
<summary>Pomicons: SIL Open Font License 1.1 (src/glyphs/pomicons/LICENSE, Nerd Fonts tag v3.4.0, commit fa7b859994228a9c8759f99c55a8d31ee92a1b5e)</summary>

```
Copyright (c) 2021, Gabriele Lana gabriele.lana@gmail.com
with Reserved Font Name Pomicons.

This Font Software is licensed under the SIL Open Font License, Version 1.1.
This license is copied below, and is also available with a FAQ at:
http://scripts.sil.org/OFL


-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded, 
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

</details>

<details>
<summary>Weather Icons: SIL Open Font License 1.1 (src/glyphs/weather-icons/OFL.txt, Nerd Fonts tag v3.4.0, commit fa7b859994228a9c8759f99c55a8d31ee92a1b5e)</summary>

```
Copyright (c) <dates>, <Erik Flowers> (<URL|email>),
with Reserved Font Name <Reserved Font Name>.
Copyright (c) <dates>, <additional Copyright Holder> (<URL|email>),
with Reserved Font Name <additional Reserved Font Name>.
Copyright (c) <dates>, <additional Copyright Holder> (<URL|email>).

This Font Software is licensed under the SIL Open Font License, Version 1.1.
This license is copied below, and is also available with a FAQ at:
http://scripts.sil.org/OFL


-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded, 
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

</details>

## Third-party: jmap-webmail, in the mail reader (MIT)

The Mail tab's reader views (`sdk/mail-reader`, bundled into the app) are adapted from **jmap-webmail** by root-fr, by way of Alakazam Labs' adaptation of it. Which files, and what changed: `sdk/mail-reader/UPSTREAM.md`.

| | |
|--|--|
| **What** | `sdk/mail-reader/src/` (types, thread grouping, the sanitizer config, the folder list, email list, thread and message views) |
| **Upstream** | [jmap-webmail](https://github.com/root-fr/jmap-webmail) v1.7.3 |
| **License** | **MIT**, text below (also in `sdk/mail-reader/NOTICE`) |

> MIT License
>
> Copyright (c) 2025 Matthieu MALVACHE
> Copyright (c) 2026 Alakazam Labs
>
> Permission is hereby granted, free of charge, to any person obtaining a copy
> of this software and associated documentation files (the "Software"), to deal
> in the Software without restriction, including without limitation the rights
> to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
> copies of the Software, and to permit persons to whom the Software is
> furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all
> copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
> IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
> FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
> AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
> LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
> OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
> SOFTWARE.

## Third-party: Stalwart mail server (AGPL-3.0-only)

Hosted mail (Email Hosting) and Email Notifications on Linux run the
**Stalwart** mail server as a separate program K2 installs and supervises.

| | |
|--|--|
| **What** | The `stalwart` server binary, version **0.16.25**, downloaded unmodified (sha256-checked) at install time from the release `v0.16.25` of [github.com/Alakazam-211/stalwart](https://github.com/Alakazam-211/stalwart), which mirrors upstream [stalwartlabs/stalwart](https://github.com/stalwartlabs/stalwart) byte for byte |
| **License** | **AGPL-3.0-only** (the Community Edition). No Enterprise licence key is ever installed |
| **Source** | The complete corresponding source is the `v0.16.25` tag: https://github.com/Alakazam-211/stalwart/tree/v0.16.25 (also printed by `k2 study email-notifications` and `k2 notifications email status`) |
| **How K2 uses it** | As a separate process over its loopback HTTP/JMAP management API only. Stalwart's code is **never linked, vendored or patched** into K2 |
| **K2 code** | Is **MIT**. Running Stalwart beside K2 as a separate program does **not** put K2 under the AGPL or otherwise relicense the product |

When redistributing K2 builds or boxes that install Stalwart, keep this
notice and the source pointer with them.

## Third-party: Zen widget library

<!-- zen-lib:begin (generated from src/shared/zen-lib.json by scripts/vendor-zen-lib.sh; do not edit by hand) -->

Libraries a Garden widget can ask for with `requires.libs` (the Zen widget
standard library). K2 inlines each one into the sealed widget frame; the frame
itself never touches the network. Each library folder keeps its full licence
text in `LICENSE.txt` (Apache-2.0 libraries include their upstream NOTICE).

| Library | Version | Licence | Copyright | Where |
|--|--|--|--|--|
| three.js (`three`) | 0.186.0 | MIT | Copyright © 2010-2026 three.js authors | shipped in the app, `src/renderer/zen-lib/three@0.186.0/` |
| globe.gl (`globe.gl`) | 2.46.2 | MIT | Copyright (c) 2019 Vasco Asturiano | shipped in the app, `src/renderer/zen-lib/globe.gl@2.46.2/` |
| cannon-es (`cannon-es`) | 0.20.0 | MIT | Copyright (c) 2015 cannon.js Authors | shipped in the app, `src/renderer/zen-lib/cannon-es@0.20.0/` |
| PixiJS (`pixi.js`) | 8.21.0 | MIT | Copyright (c) 2013-2023 Mathew Groves, Chad Engler | shipped in the app, `src/renderer/zen-lib/pixi.js@8.21.0/` |
| Phaser (`phaser`) | 4.2.1 | MIT | Copyright (c) 2026 Richard Davey, Phaser Studio Inc. | shipped in the app, `src/renderer/zen-lib/phaser@4.2.1/` |
| KAPLAY (`kaplay`) | 3001.0.19 | MIT | Copyright (c) 2025, KAPLAY Team and contributers | shipped in the app, `src/renderer/zen-lib/kaplay@3001.0.19/` |
| Matter.js (`matter-js`) | 0.20.0 | MIT | Copyright (c) Liam Brummitt and contributors. | shipped in the app, `src/renderer/zen-lib/matter-js@0.20.0/` |
| D3 (`d3`) | 7.9.0 | ISC | Copyright 2010-2023 Mike Bostock | shipped in the app, `src/renderer/zen-lib/d3@7.9.0/` |
| Chart.js (`chart.js`) | 4.5.1 | MIT | Copyright (c) 2014-2024 Chart.js Contributors | shipped in the app, `src/renderer/zen-lib/chart.js@4.5.1/` |
| Apache ECharts (`echarts`) | 6.1.0 | Apache-2.0 | Copyright 2017-2026 The Apache Software Foundation; This product includes software developed at The Apache Software Foundation (https://www.apache.org/). | shipped in the app, `src/renderer/zen-lib/echarts@6.1.0/` |
| uPlot (`uplot`) | 1.6.32 | MIT | Copyright (c) 2022 Leon Sorokin | shipped in the app, `src/renderer/zen-lib/uplot@1.6.32/` |
| Anime.js (`animejs`) | 4.5.0 | MIT | Copyright (c) 2025 Julian Garnier | shipped in the app, `src/renderer/zen-lib/animejs@4.5.0/` |
| Lottie (`lottie-web`) | 5.13.0 | MIT | Copyright (c) 2015 Bodymovin | shipped in the app, `src/renderer/zen-lib/lottie-web@5.13.0/` |
| perfect-freehand (`perfect-freehand`) | 1.2.3 | MIT | Copyright (c) 2021 Stephen Ruiz Ltd | shipped in the app, `src/renderer/zen-lib/perfect-freehand@1.2.3/` |
| Rough.js (`roughjs`) | 4.6.6 | MIT | Copyright (c) 2019 Preet Shihn | shipped in the app, `src/renderer/zen-lib/roughjs@4.6.6/` |
| howler.js (`howler`) | 2.2.4 | MIT | Copyright (c) 2013-2020 James Simpson and GoldFire Studios, Inc. | shipped in the app, `src/renderer/zen-lib/howler@2.2.4/` |
| Tone.js (`tone`) | 15.1.22 | MIT | Copyright (c) 2014-2020 Yotam Mann | shipped in the app, `src/renderer/zen-lib/tone@15.1.22/` |
| Leaflet (`leaflet`) | 1.9.4 | BSD-2-Clause | Copyright (c) 2010-2023, Volodymyr Agafonkin | shipped in the app, `src/renderer/zen-lib/leaflet@1.9.4/` |
| marked (`marked`) | 18.0.14 | MIT | Copyright (c) 2018+, MarkedJS (https://github.com/markedjs/) | shipped in the app, `src/renderer/zen-lib/marked@18.0.14/` |
| DOMPurify (`dompurify`) | 3.4.17 | Apache-2.0 | (c) Cure53 and other contributors | shipped in the app, `src/renderer/zen-lib/dompurify@3.4.17/` |
| highlight.js (`highlight.js`) | 11.12.0 | BSD-3-Clause | Copyright (c) 2006, Ivan Sagalaev. | shipped in the app, `src/renderer/zen-lib/highlight.js@11.12.0/` |
| KaTeX (`katex`) | 0.18.9 | MIT | Copyright (c) 2013-2020 Khan Academy and other contributors | shipped in the app, `src/renderer/zen-lib/katex@0.18.9/` |
| Preact + htm (`preact`) | 10.29.8 | MIT AND Apache-2.0 | Copyright (c) 2015-present Jason Miller; htm: Copyright 2018 Google Inc. | shipped in the app, `src/renderer/zen-lib/preact@10.29.8/` |
| Lit (`lit`) | 3.3.3 | BSD-3-Clause | Copyright (c) 2017 Google LLC. All rights reserved. | shipped in the app, `src/renderer/zen-lib/lit@3.3.3/` |
| Caveat (handwriting font) (`font-caveat`) | 5.3.0 | OFL-1.1 | Copyright 2014 The Caveat Project Authors (https://github.com/googlefonts/caveat) | shipped in the app, `src/renderer/zen-lib/font-caveat@5.3.0/` |
| Babylon.js (`babylonjs`) | 8.52.1 | Apache-2.0 | Copyright 2023 The Babylon.js team | downloaded on first use by this computer's K2 (not shipped); licence kept at `src/renderer/zen-lib/babylonjs@8.52.1/LICENSE.txt` |
| Plotly (`plotly.js-dist-min`) | 4.1.1 | MIT | Copyright (c) 2016-2024 Plotly Technologies Inc. | downloaded on first use by this computer's K2 (not shipped); licence kept at `src/renderer/zen-lib/plotly.js-dist-min@4.1.1/LICENSE.txt` |
| K2 help button (widget kit) (`k2-help`) | 1.0.1 | MIT | Copyright (c) 2025-2026 Alakazam Labs | shipped in the app, `src/renderer/zen-lib/k2-help@1.0.1/` |
| K2 activity bar (widget kit) (`k2-activity`) | 1.0.1 | MIT | Copyright (c) 2025-2026 Alakazam Labs | shipped in the app, `src/renderer/zen-lib/k2-activity@1.0.1/` |
| K2 Thread panel (widget kit) (`k2-thread`) | 1.1.1 | MIT | Copyright (c) 2025-2026 Alakazam Labs | shipped in the app, `src/renderer/zen-lib/k2-thread@1.1.1/` |
| K2 markdown (widget kit) (`k2-markdown`) | 1.0.1 | MIT | Copyright (c) 2025-2026 Alakazam Labs | shipped in the app, `src/renderer/zen-lib/k2-markdown@1.0.1/` |
| K2 PNG encoder (widget kit) (`k2-png`) | 1.0.1 | MIT | Copyright (c) 2025-2026 Alakazam Labs | shipped in the app, `src/renderer/zen-lib/k2-png@1.0.1/` |
| K2 diff view (widget kit) (`k2-diff`) | 1.0.1 | MIT | Copyright (c) 2025-2026 Alakazam Labs | shipped in the app, `src/renderer/zen-lib/k2-diff@1.0.1/` |

K2 code is **MIT**. These libraries keep their own
licences and do **not** relicense K2.

<!-- zen-lib:end -->
<!-- END NOTICE.md -->
