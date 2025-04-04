//! Commandline tool to interact with WebDAV servers.
#![allow(clippy::needless_pass_by_value)]
use std::collections::HashMap;
use std::env;
use std::fmt::Write as _;
use std::io::{stdin, stdout};
use std::num::ParseIntError;
use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;

use anyhow::{Context as _, Error, Result, anyhow, bail};
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use comfy_table::presets::{NOTHING, UTF8_FULL_CONDENSED};
use comfy_table::{ContentArrangement, Table};
use derive_more::Error;
use derive_more::derive::{Display, FromStr};
use humansize::DECIMAL;
use percent_encoding::percent_decode;
use reqwest::blocking::Body;
use time::OffsetDateTime;
use time::format_description::well_known::{Rfc2822, Rfc3339};
use webdav_client::webdav_types::{PropValue, Response};
use webdav_client::{Auth, Depth, Request};

#[derive(Debug, Error, Display)]
#[display("{_0}")]
struct ExitCodeError(u8, Error);

impl From<Error> for ExitCodeError {
    fn from(value: Error) -> Self {
        Self(1, value)
    }
}

#[derive(Clone, Debug)]
struct Client {
    inner: webdav_client::Client<reqwest::blocking::Client>,
    host: String,
}

impl Client {
    fn new(host: String, login: Option<String>, password: Option<Password>) -> Self {
        let auth = if login.is_some() {
            Auth::Basic {
                username: login.unwrap_or_default(),
                password: password.map(|p| p.0),
            }
        } else {
            Auth::None
        };

        Self {
            inner: webdav_client::Client::authenticated(reqwest::blocking::Client::new(), auth),
            host,
        }
    }

    fn path(&self, path: &str) -> String {
        if self.host.ends_with('/') && path.starts_with('/') {
            self.host.clone() + path.trim_start_matches('/')
        } else if self.host.ends_with('/') || path.starts_with('/') {
            self.host.clone() + path
        } else {
            self.host.clone() + "/" + path
        }
    }

    fn list(&self, path: &str, depth: Depth, fields: &[ListField]) -> Result<()> {
        let mut namespaces = HashMap::new();
        let names = fields
            .iter()
            .flat_map(|field| field.to_xml(&mut namespaces))
            .collect::<Result<Vec<String>>>()?;
        let namespaces: Vec<_> = namespaces
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .chain([
                ("d", "DAV:"),
                ("oc", "http://owncloud.org/ns"),
                ("nc", "http://nextcloud.org/ns"),
            ])
            .collect();
        let url = self.path(path);
        let xml = self.inner.prop_find(&url, depth, &names, namespaces);
        if let Err(e) = xml {
            if e.is_not_found() {
                bail!(ExitCodeError(
                    44,
                    anyhow!("404 Does not exist {}", self.path(path))
                ))
            }
            bail!(e)
        };
        let xml = xml?;

        let mut table = Table::new();
        if table.is_tty() {
            table.load_preset(UTF8_FULL_CONDENSED);
            table.set_content_arrangement(ContentArrangement::Dynamic);
        } else {
            table.load_preset(NOTHING);
        }
        table.set_header(fields);
        for line in xml {
            table.add_row(
                fields
                    .iter()
                    .map(|field| field.extract(&line, &url).unwrap_or_default()),
            );
        }

        println!("{table}");
        Ok(())
    }

    fn get(&self, path: String, out_path: Option<PathBuf>) -> Result<()> {
        let result = self.inner.get_raw(self.path(&path));
        match result {
            Ok(mut result) => {
                if let Some(out_path) = out_path {
                    result.copy_to(&mut std::fs::File::create_new(&out_path).with_context(
                        || format!("Could not create file for output `{}`", out_path.display()),
                    )?)?;
                } else {
                    result.copy_to(&mut stdout())?;
                }
                Ok(())
            }
            Err(e) if e.is_not_found() => bail!("404 Not Found {}", self.path(&path)),
            other => {
                other?;
                Ok(())
            }
        }
    }

    fn put(&self, path: String, in_path: Option<PathBuf>) -> Result<()> {
        let request = self.inner.put_raw(self.path(&path));
        if let Err(e) = Request::send_ok(
            if let Some(in_path) = in_path {
                request.body(std::fs::File::open(&in_path).with_context(|| {
                    format!("Could not read input file `{}`", in_path.display())
                })?)
            } else {
                request.body(Body::new(stdin()))
            },
            None,
        ) {
            if e.is_conflict() {
                bail!("409 Conflict (probably a directory) {}", self.path(&path))
            }
            if e.is_not_found() {
                bail!(ExitCodeError(
                    44,
                    anyhow!(
                        "404 Not Found (probably parent directory non-existent) {}",
                        self.path(&path)
                    )
                ))
            }
            bail!(e)
        };
        Ok(())
    }
}

fn replace_env(mut help: String) -> String {
    fn shorten(s: String) -> String {
        let max_len = 32;
        if s.chars().count() > max_len {
            s.chars()
                .take(max_len / 2)
                .chain(['…'])
                .chain(s.chars().skip(s.len() - max_len / 2))
                .collect()
        } else {
            s
        }
    }
    for env in ["LOGIN", "HOST"] {
        help = help.replace(
            &format!("{{{env}}}"),
            &env::var(env).map(shorten).unwrap_or_default(),
        );
    }
    help = help.replace(
        "{PASSWORD}",
        &env::var("PASSWORD")
            .map(|password| format!("{:?}", Password(password)))
            .map(shorten)
            .unwrap_or_default(),
    );
    help
}

fn main() -> Result<ExitCode> {
    let command = Args::command_for_update().mut_args(|mut a| {
        if let Some(help) = a.get_help().map(|s| s.ansi().to_string()) {
            a = a.help(replace_env(help.to_string()));
        }
        if let Some(help) = a.get_long_help().map(|s| s.ansi().to_string()) {
            a = a.help(replace_env(help.to_string()));
        }
        a
    });
    let Args {
        login,
        password,
        host,
        action,
    } = Args::from_arg_matches(&command.get_matches())?;

    let client = Client::new(host, login, password);

    if let Err(e) = match action {
        Action::Get { path, out_path } => client.get(path, out_path),
        Action::Put { path, in_path } => client.put(path, in_path),
        Action::Delete => todo!(),
        Action::Mkcol => todo!(),
        Action::Move => todo!(),
        Action::Copy => todo!(),
        Action::List {
            path,
            depth,
            mut fields,
            extra_fields,
        } => {
            fields.extend_from_slice(&extra_fields);
            client.list(&path, depth, &fields)
        }
    } {
        let ExitCodeError(code, error) = e.downcast::<ExitCodeError>()?;
        eprintln!("{error:?}");
        Ok(code.into())
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

#[derive(Clone, FromStr, Default)]
struct Password(String);

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{1:*<0$}", self.0.len(), "")
    }
}

/// Utility to interact with WebDAV servers.
#[derive(Parser, Debug)]
struct Args {
    /// Username used for authentication [env LOGIN={LOGIN}]
    #[arg(short, long, env, hide_env = true)]
    login: Option<String>,
    /// Password used for authentication [env PASSWORD={PASSWORD}]
    #[arg(short, long, env, hide_env = true)]
    password: Option<Password>,
    /// WebDAV server to use [env HOST={HOST}]
    #[arg(short = 's', long, env, hide_env = true)]
    host: String,
    #[clap(subcommand)]
    action: Action,
}

#[derive(Subcommand, Debug)]
enum Action {
    Get {
        #[clap(default_value = "/")]
        path: String,
        #[clap(long, short)]
        out_path: Option<PathBuf>,
    },
    Put {
        #[clap(default_value = "/")]
        path: String,
        #[clap(long, short)]
        in_path: Option<PathBuf>,
    },
    Delete,
    #[clap(alias = "mkdir")]
    Mkcol,
    Move,
    Copy,
    /// List files and their properties.
    List {
        #[clap(default_value = "/")]
        path: String,
        #[clap(long, short, value_parser = parse_depth, default_value = "1")]
        depth: Depth,
        /// The fields to request and show for each entry.
        ///
        /// Supports some abstraction for common ones, but also allows
        /// specifying "raw" parameters for the `PROPFIND` request, e.g.
        /// `d:getetag`.
        ///
        /// # Predefined Fields
        ///
        /// - `name` (`d:displayname`)
        /// - `created-at` (`d:creationdate`)
        /// - `modified-at` (`d:getlastmodified`)
        /// - `content-type` (`d:getcontenttype`)
        /// - `resourece-type` (`d:resourcetype`)
        /// - `content-length` File size in bytes (`d:getcontentlength`).
        /// - `size` Human-readable size `d:getcontentlength` for files and
        ///   `oc:size` for folders.
        /// - `folder-entry-count` Sum of `nc:contained-fodler-count` and
        ///   `nc:contained-file-count`
        /// - `tags` Tags both from `oc:tags` and `nc:system-tags`
        /// - `owner-id` (`oc:owner-id`)
        /// - `owner-name` (`oc:owner-name`)
        ///
        /// # Predefined Namespaces
        ///
        /// - `d="DAV:"`
        /// - `oc="http://owncloud.org/ns"`
        /// - `nc="http://nextcloud.org/ns"`
        /// - `ocs="http://open-collaboration-services.org/ns"`
        /// - `ocm="http://open-cloud-mesh.org/ns"`
        ///
        /// Other namespaces can be used by specifying them in braces
        /// (`{name.space/uri}tag-name`), but also feel free to request
        /// adding them at <https://github.com/ModProg/webdav-client>.
        #[clap(
            long,
            short,
            default_value = "path,modified-at,size",
            value_delimiter = ',',
            value_parser = parse_list_fields,
        )]
        fields: Vec<ListField>,
        /// Like `--fields` but appends the fields to the default list
        #[clap(
            long,
            short = 'F',
            value_delimiter = ',',
            value_parser = parse_list_fields,
        )]
        extra_fields: Vec<ListField>,
    },
}

/// E.g. for Nextcloud: <https://docs.nextcloud.com/server/latest/developer_manual/client_apis/WebDAV/basic.html#supported-properties>
#[derive(ValueEnum, Clone, Debug, Display)]
#[display(rename_all = "kebab-case")]
enum ListField {
    AbsolutePath,
    Path,
    /// `d:displayname`
    Name,
    /// `d:creationdate`
    CreatedAt,
    /// `d:getlastmodified`
    ModifiedAt,
    /// `d:getcontenttype`
    ContentType,
    // `d:resourcetype`
    ResourceType,
    /// File size in bytes `d:getcontentlength`
    ContentLength,
    /// Human-readable size `d:getcontentlength` for files and `oc:size` for
    /// folders
    Size,
    /// `nc:contained-folder-count` plus `nc:contained-file-count`
    FolderEntryCount,
    /// `oc:tags` & `nc:system-tags`
    Tags,
    /// `oc:owner-id`
    OwnerId,
    /// `oc:owner-display-name`
    OwnerName,
    #[clap(skip)]
    #[display("{name}")]
    Other {
        namespace_uri: Option<String>,
        name: String,
    },
}

#[allow(clippy::unnecessary_wraps)]
fn parse_list_fields(value: &str) -> Result<ListField> {
    Ok(match ListField::from_str(value, true) {
        Ok(o) => o,
        Err(_) => {
            if let Some(value) = value.strip_prefix('{') {
                if let Some((namespace_uri, name)) = value.rsplit_once('}') {
                    ListField::Other {
                        namespace_uri: Some(namespace_uri.to_owned()),
                        name: name.to_owned(),
                    }
                } else {
                    bail!("Expected closing `}}`");
                }
            } else {
                ListField::Other {
                    namespace_uri: None,
                    name: value.to_owned(),
                }
            }
        }
    })
}

impl ListField {
    fn to_xml(&self, namespaces: &mut HashMap<String, String>) -> Vec<anyhow::Result<String>> {
        vec![Ok(match self {
            ListField::Path | ListField::AbsolutePath => return vec![],
            ListField::Name => "d:displayname",
            ListField::CreatedAt => "d:creationdate",
            ListField::ModifiedAt => "d:getlastmodified",
            ListField::ContentType => "d:getcontenttype",
            ListField::ResourceType => "d:resourcetype",
            ListField::ContentLength => "d:getcontentlength",
            ListField::Size => {
                return vec![
                    Ok("d:getcontentlength".to_owned()),
                    Ok("oc:size".to_owned()),
                ];
            }
            ListField::FolderEntryCount => {
                return vec![
                    Ok("nc:contained-folder-count".to_owned()),
                    Ok("nc:contained-file-count".to_owned()),
                ];
            }
            ListField::Tags => {
                return vec![Ok("oc:tags".to_owned()), Ok("nc:system-tags".to_owned())];
            }
            ListField::OwnerId => "oc:owner-id",
            ListField::OwnerName => "oc:owner-display-name",
            ListField::Other {
                namespace_uri,
                name,
            } => {
                if let Some(namespace_uri) = namespace_uri {
                    let len = namespaces.len();
                    let namespace_name = namespaces
                        .entry(namespace_uri.to_owned())
                        .or_insert_with(|| format!("webdav-client-ns{len}"));
                    return vec![Ok(format!("{namespace_name}:name"))];
                }
                name
            }
        }
        .to_owned())]
    }

    #[allow(clippy::too_many_lines)]
    fn extract(&self, response: &Response, prefix: &str) -> Option<String> {
        if let Self::AbsolutePath = self {
            Some(response.href.clone())
        } else if let Self::Path = self {
            let response = 'response: {
                let mut prefix = prefix.trim_end_matches('/');
                if let Some(response) = response.href.strip_prefix(prefix) {
                    break 'response response;
                };
                while !prefix.is_empty() {
                    if let Some(response) = response
                        .href
                        .strip_prefix(prefix)
                        .or_else(|| response.href.strip_prefix(prefix.strip_prefix('/')?))
                    {
                        break 'response response;
                    }
                    prefix = prefix.strip_prefix('/').unwrap_or(prefix);
                    if let Some(index) = prefix.find('/') {
                        prefix = &prefix[index..];
                        continue;
                    }
                    break;
                }
                &response.href
            }
            .trim_start_matches('/');
            if response.is_empty() {
                return Some(".".to_owned());
            }
            percent_decode(response.as_bytes())
                .decode_utf8()
                .map(String::from)
                .ok()
                .or_else(|| Some(response.to_string()))
        } else {
            let successful = response
                .propstat
                .iter()
                .find(|ps| ps.status.is_successful())?;

            let ref_value = |name: &str| successful.prop.get(name)?.try_unwrap_text_ref().ok();
            let get_value = |name: &str| ref_value(name).cloned();
            let get_number = |name: &str| {
                usize::from_str(ref_value(name).or_else(|| ref_value("getcontentlength"))?).ok()
            };

            let parse_date = |name: &str| {
                let value = ref_value(name)?;
                Some(
                    OffsetDateTime::parse(value, &Rfc3339)
                        .or_else(|_| OffsetDateTime::parse(value, &Rfc2822))
                        .map_or_else(
                            |_| value.clone(),
                            |date| date.format(&Rfc3339).unwrap_or_else(|_| value.clone()),
                        ),
                )
            };

            fn to_xml(prop_value: &PropValue, out: &mut String) {
                match prop_value {
                    PropValue::Empty => {}
                    PropValue::Text(text) => out.push_str(text),
                    PropValue::Xml(hash_map) => {
                        for (name, value) in hash_map {
                            for value in value {
                                write!(out, "<{name}>").unwrap();
                                to_xml(value, out);
                                write!(out, "</{name}>").unwrap();
                            }
                        }
                    }
                }
            }

            match self {
                ListField::AbsolutePath | ListField::Path => unreachable!(),
                ListField::Name => todo!(),
                ListField::CreatedAt => parse_date("creationdate"),
                ListField::ModifiedAt => parse_date("getlastmodified"),
                ListField::ContentType => get_value("getcontenttype"),
                ListField::ResourceType => get_value("resourcetype"),
                ListField::ContentLength => get_value("getcontentlength"),
                ListField::Size => Some(humansize::format_size(
                    get_number("size").or_else(|| get_number("getcontentlength"))?,
                    DECIMAL,
                )),
                ListField::FolderEntryCount => Some(
                    (get_number("contained-file-count").unwrap_or_default()
                        + get_number("contained-folder-count").unwrap_or_default())
                    .to_string(),
                ),
                ListField::Tags => {
                    let system_tags = successful
                        .prop
                        .get("system-tags")
                        .and_then(|tags| {
                            tags.try_unwrap_xml_ref()
                                .ok()?
                                .get("system-tag")
                                .map(Vec::as_slice)
                        })
                        .unwrap_or_default();
                    let tags = successful
                        .prop
                        .get("tags")
                        .and_then(|tags| {
                            tags.try_unwrap_xml_ref()
                                .ok()?
                                .get("tag")
                                .map(Vec::as_slice)
                        })
                        .unwrap_or_default();
                    Some(
                        system_tags
                            .iter()
                            .chain(tags)
                            .fold(String::new(), |mut out, tag| {
                                if !out.is_empty() {
                                    out += ",";
                                }
                                if let Ok(text) = tag.try_unwrap_text_ref() {
                                    out += text;
                                } else {
                                    to_xml(tag, &mut out);
                                }
                                out
                            }),
                    )
                }
                ListField::OwnerId => get_value("owner-id"),
                ListField::OwnerName => get_value("owner-display-name"),
                ListField::Other { name, .. } => {
                    let mut out = String::new();
                    to_xml(
                        successful
                            .prop
                            .get(name.split_once(':').map_or(name.as_str(), |name| name.1))?,
                        &mut out,
                    );
                    Some(out)
                }
            }
        }
    }
}

// impl Display for ListField {}

fn parse_depth(value: &str) -> Result<Depth, ParseIntError> {
    if value.len() >= 3 && "infinity".starts_with(&value.to_lowercase()) {
        Ok(Depth::Infinity)
    } else {
        Ok(Depth::Some(value.parse()?))
    }
}
