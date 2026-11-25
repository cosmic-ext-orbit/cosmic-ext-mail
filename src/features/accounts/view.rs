// SPDX-License-Identifier: GPL-3.0-only

use accounts::{
    Provider, Service,
    ui::{
        AccountRow, Form as AccountsForm, FormState, Labels, View as AccountsView, provider_label,
    },
};
use cosmic::Element;
use uuid::Uuid;

use crate::fl;

use super::{Accounts, Message};

/// Mail's account page is the shared embedded Accounts panel. The provider
/// picker contains Google, Microsoft, and the library's custom IMAP/SMTP form;
/// Mail no longer has a separate local account editor.
pub fn view(app: &Accounts) -> Element<'_, Message> {
    let rows: Vec<AccountRow<'_>> = app
        .managed
        .iter()
        .map(|account| AccountRow {
            id: account.id,
            provider: account.provider,
            label: account.email_or_identity().to_owned().into(),
            detail: provider_label(account.provider).into(),
            enabled: account.enabled,
            status: None,
        })
        .collect();

    let labels = Labels {
        title: fl!("online-accounts").into(),
        checking: fl!("account-loading").into(),
        empty: fl!("account-empty").into(),
        no_providers: fl!("accounts-no-providers").into(),
        add: fl!("account-add").into(),
        add_title: fl!("accounts-add-title").into(),
        add_body: fl!("accounts-add-body").into(),
        back: fl!("account-back").into(),
        finish_title: fl!("accounts-finish-title").into(),
        finish_body: &finish_body,
        open_browser_again: fl!("accounts-open-browser").into(),
        cancel: fl!("account-cancel").into(),
        reconnect: fl!("accounts-reconnect").into(),
        remove: fl!("accounts-remove").into(),
        sync: "Sync".into(),
        url: fl!("account-server").into(),
        email: fl!("account-email").into(),
        username: fl!("account-username").into(),
        password: fl!("accounts-password").into(),
        imap_host: fl!("account-imap-host").into(),
        imap_port: fl!("account-imap-port").into(),
        smtp_host: fl!("account-smtp-host").into(),
        smtp_port: fl!("account-smtp-port").into(),
        starttls: fl!("account-starttls").into(),
        connect: fl!("accounts-connect").into(),
        sign_in: &provider_action,
    };

    let form = app.custom_form.as_ref().map(|form| AccountsForm {
        state: FormState {
            provider: Provider::CustomImapSmtp,
            service: Service::Mail,
            url: "",
            email: &form.email,
            username: &form.username,
            password: &form.password.0,
            imap_host: &form.imap_host,
            imap_port: &form.imap_port,
            smtp_host: &form.smtp_host,
            smtp_port: &form.smtp_port,
            base_dn: "",
            starttls: form.smtp_starttls,
        },
        on_change: &change_field,
        on_starttls: Some(&toggle_starttls),
        on_submit: &submit_custom,
        on_cancel: &cancel_custom,
    });

    accounts::ui::view(AccountsView {
        service: Service::Mail,
        accounts: rows,
        pending: app.auth.as_ref(),
        loading: app.discovering,
        adding: app.adding_online,
        providers: None,
        form,
        footer: None,
        labels,
        on_add_open: &open_add,
        on_add_close: &close_add,
        on_open_url: &open_browser,
        on_add: &add_provider,
        on_cancel: &cancel_auth,
        on_remove: &remove,
        on_reconnect: &reconnect,
        on_toggle: &toggle_link,
        on_sync: None,
    })
}

fn finish_body(provider: Provider) -> String {
    fl!("accounts-finish-body", provider = provider_label(provider))
}

fn provider_action(provider: Provider) -> String {
    match provider {
        Provider::CustomImapSmtp => fl!("accounts-custom-imap-smtp"),
        provider => fl!("accounts-sign-in", provider = provider_label(provider)),
    }
}

fn open_add() -> Message {
    Message::AddOnlineOpen
}

fn close_add() -> Message {
    Message::AddOnlineClose
}

fn open_browser() -> Message {
    Message::OpenBrowser
}

fn add_provider(provider: Provider) -> Message {
    match provider {
        Provider::CustomImapSmtp => Message::AddCustomImapSmtp,
        provider => Message::AddProvider(provider),
    }
}

fn cancel_auth(state: &str) -> Message {
    Message::CancelAuth(state.to_owned())
}

fn reconnect(id: Uuid) -> Message {
    Message::Reconnect(id)
}

fn remove(id: Uuid) -> Message {
    Message::RemoveOnline(id)
}

fn toggle_link(id: Uuid, enabled: bool) -> Message {
    Message::ToggleLink(id, enabled)
}

fn change_field(field: accounts::ui::FormField, value: String) -> Message {
    Message::CustomFieldChanged(field, value)
}

fn toggle_starttls(enabled: bool) -> Message {
    Message::CustomStartTlsChanged(enabled)
}

fn submit_custom() -> Message {
    Message::SubmitCustomImapSmtp
}

fn cancel_custom() -> Message {
    Message::CancelCustomForm
}
