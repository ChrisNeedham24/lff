# lff

A fast and simple recursive 'large file finder', no pipes required.

### Usage

Run `lff -h/--help` or see below.

```
Usage: lff [OPTIONS] <DIRECTORY>

Arguments:
  <DIRECTORY>  The directory to begin searching in

Options:
  -a, --absolute                     Display absolute paths for files. Automatically true if the supplied directory isn't relative
      --base-ten                     Whether to display file sizes in KB/MB/GB over KiB/MiB/GiB
      --exclude-hidden               Exclude hidden files and directories
  -e, --extension <EXTENSION>        Filter files by extension
  -l, --limit <LIMIT>                Return a maximum of this many files
  -m, --min-size-mib <MIN_SIZE_MIB>  The minimum size in MiB for displayed files, e.g. 10 = 10 MiB, 0.1 = 100 KiB [default: 50]
  -n, --name-pattern <NAME_PATTERN>  Filter file names by glob patterns, e.g. *abc* will yield 1abc2.txt
  -p, --pretty                       Pretty-prints file sizes
  -s, --sort-method <SORT_METHOD>    How to sort found files [possible values: size, name]
  -h, --help                         Print help
  -V, --version                      Print version
```

**Hint**: to see all files in a directory, just pass `-m 0`.

### Installation

TBC

### Benchmarks

These benchmarks are run using [hyperfine](https://github.com/sharkdp/hyperfine),
with 10 warmup runs and 20 actual runs each, on an iMac (24-inch, M1, 2021).

All benchmarks are run against the master branch of the [Linux source tree](https://github.com/torvalds/linux).

In these commands, `linux-source` is the name of the downloaded folder.

These benchmarks compare `lff` to the GNU `du` tool and [dust](https://github.com/bootandy/dust),
which is a very cool and extended Rust version of `du`.

#### 100 largest files

| Command                                           |   Mean [ms] | Min [ms] | Max [ms] |    Relative |
|:--------------------------------------------------|------------:|---------:|---------:|------------:|
| `lff -m 0 -s size -l 100 linux-source`            | 113.6 ± 8.0 |    110.1 |    147.0 |        1.00 |
| `du -a linux-source \| sort -r -n \| head -n 100` | 343.0 ± 9.6 |    336.1 |    378.6 | 3.02 ± 0.23 |
| `dust --skip-total -R -F -n 100 -r linux-source`  | 133.7 ± 8.5 |    129.3 |    169.0 | 1.18 ± 0.11 |

#### Entire repository sorted by size

| Command                                             |    Mean [ms] | Min [ms] | Max [ms] |    Relative |
|:----------------------------------------------------|-------------:|---------:|---------:|------------:|
| `lff -m 0 -s size linux-source`                     | 221.4 ± 13.0 |    210.8 |    260.7 |        1.00 |
| `du -a linux-source \| sort -r -n`                  | 412.1 ± 13.4 |    401.9 |    453.2 | 1.86 ± 0.12 |
| `dust --skip-total -R -F -n 100000 -r linux-source` | 862.4 ± 18.5 |    846.1 |    906.7 | 3.89 ± 0.24 |

#### Entire repository unsorted

| Command                 |    Mean [ms] | Min [ms] | Max [ms] |    Relative |
|:------------------------|-------------:|---------:|---------:|------------:|
| `lff -m 0 linux-source` | 205.9 ± 14.2 |    192.8 |    255.1 |        1.00 |
| `du -a linux-source`    | 242.6 ± 11.1 |    236.1 |    280.7 | 1.18 ± 0.10 |

NB: `dust` does not allow for unsorted queries.

#### First 100 files in repository, unsorted

| Command                             |  Mean [ms] | Min [ms] | Max [ms] |     Relative |
|:------------------------------------|-----------:|---------:|---------:|-------------:|
| `lff -m 0 -l 100 linux-source`      | 80.1 ± 7.3 |     74.1 |    104.8 | 10.08 ± 3.91 |
| `du -a linux-source \| head -n 100` |  7.9 ± 3.0 |      5.9 |     19.1 |         1.00 |

NB: `dust` does not allow for unsorted queries.

#### Entire repository, no hidden files

| Command                                                |    Mean [ms] | Min [ms] | Max [ms] |    Relative |
|:-------------------------------------------------------|-------------:|---------:|---------:|------------:|
| `lff -m 0 -s size --exclude-hidden linux-source`       | 223.7 ± 13.5 |    209.5 |    255.5 |        1.00 |
| `dust --skip-total -R -F -n 100000 -r -i linux-source` | 864.8 ± 19.1 |    843.8 |    900.1 | 3.87 ± 0.25 |

NB: `du` does not have a flag to ignore hidden files - we could provide a mask, but that would not be fair.

#### All files larger than 1 MiB, sorted by size

| Command                                                        |   Mean [ms] | Min [ms] | Max [ms] |    Relative |
|:---------------------------------------------------------------|------------:|---------:|---------:|------------:|
| `lff -m 1 -s size linux-source`                                | 98.7 ± 10.6 |     91.9 |    140.4 |        1.00 |
| `dust --skip-total -R -F -n 100000 -r -z 1048576 linux-source` | 132.5 ± 5.1 |    126.0 |    140.4 | 1.34 ± 0.15 |

NB: `du`'s `-t/--threshold` flag doesn't seem to work on macOS...

#### All files matching the pattern `*init.c` (glob) or `.*init\.c` (regex), sorted by size

| Command                                                         |    Mean [ms] | Min [ms] | Max [ms] |    Relative |
|:----------------------------------------------------------------|-------------:|---------:|---------:|------------:|
| `lff -m 0 -s size -n *init.c linux-source`                      | 356.2 ± 14.0 |    327.4 |    379.8 | 2.75 ± 0.34 |
| `dust --skip-total -R -F -n 100000 -r -e .*init.c linux-source` | 129.7 ± 15.5 |    118.7 |    173.5 |        1.00 |

NB: `du`'s `-I/--mask` flag doesn't seem to work on macOS...
