# Future Homes Standard (FHS) wrapper for the Home Energy Model (HEM) written in Rust

## Overview

The Future Homes Standard (FHS) is a wrapper around the Home Energy Model (HEM) - 
the UK Government’s proposed National Calculation Methodology for assessing the energy performance of dwellings. 
FHS deals with assessing compliance with Part L of the building regulations and can do so by creating an actual 
and notional building which are both assessed using the core HEM and corresponding FHS postprocessing steps.

Please note that the FHS wrapper is currently in development and should not be used for any official purpose.

This project constitutes a port into Rust of
the wrapper written in Python commissioned by DESNZ.

## Running the wrapper

### From CLI

Requires the `rustup` toolchain to use. ([Instructions](https://rustup.rs) for installation. For macOS, don't use
Homebrew to install Rust.)

To run tests:

```bash
cargo test
```

Running the engine requires an input JSON file that conforms to the [FHS input schema](schema/input_fhs.schema.json).

Example input files for demonstration purposes can be found at 
[examples/input/future_homes_standard](examples/input/future_homes_standard).

#### FHS Compliance mode

The default way to run the FHS wrapper is to run the `fhs-compliance` mode i.e. to run all of:

* The actual building
* The actual building with FEE assumptions
* The notional building
* The notional building with FEE assumptions

This will return an FHS compliance response.

To do this, run:

```
cargo run --release --features="clap indicatif" -- path/to/input.json
```

#### Individual modes

Any of the individual modes listed above may be run using the `--mode` option:

* The actual building: `--mode actual`
* The actual building with FEE assumptions: `--mode actual-FEE`
* The notional building: `--mode notional`
* The notional building with FEE assumptions: `--mode notional-FEE`

The individual modes can be combined, e.g.:

```
cargo run --release --features="clap indicatif" -- path/to/input.json --mode "actual actual-FEE"
```

#### Weather file option

The wrapper uses a representative weather file by default but there is also the option to provide a weather file, for example:

```bash
cargo run --release --features="clap indicatif" -- path/to/input.json -e path/to/weather_file.epw
```

#### Core output formats

Optionally, the core output format can be specified. Valid values are `csv` (default), `json` or `json csv`: 

```
cargo run --release --features="clap indicatif" -- path/to/input.json --core-output-formats "json"
```

The `clap` feature above is mandatory. The `indicatif` feature switches on the output of a progress bar.

## Contributing

### Using the commit template

If you've done work in a pair or ensemble why not add your co-author(s) to the commit? This way everyone involved is
given credit and people know who they can approach for questions about specific commits. To make this easy there is a
commit template with a list of regular contributors to this code base. You will find it at the root of this
project: `commit_template.txt`. Each row represents a possible co-author, however everyone is commented out by default (
using `#`), and any row that is commented out will not show up in the commit.

#### Editing the template

If your name is not in the `commit_template.txt` yet, edit the file and add a new row with your details, following the
format `#Co-Authored-By: Name <email>`, e.g. `#Co-Authored-By: Maja <maja@gmail.com>`. The email must match the email
you use for your GitHub account. To protect your privacy, you can activate and use your noreply GitHub addresses (find
it in GitHub under Settings > Emails > Keep my email addresses private).

#### Getting set up

To apply the commit template navigate to the root of the project in a terminal and
use: `git config commit.template commit_template.txt`. This will edit your local git config for the project and apply
the template to every future commit.

#### Using the template (committing with co-authors)

When creating a new commit, edit your commit (e.g. using vim, or a code editor) and delete the `#` in front of any
co-author(s) you want to credit. This means that it's probably easier and quicker to use `git commit` (instead
of `git commit -m ""` followed by a `git commit --amend`), as it will show you the commit template content for you to
edit.
