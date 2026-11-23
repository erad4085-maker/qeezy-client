use freya::prelude::*;
use oneclient_auth::MinecraftAccount;

use crate::components::{Avatar, Button, Icon, IconType, use_microsoft_login};
use crate::hooks::{try_default_account, use_current_account};
use crate::i18n::t;
use crate::routes::Route;
use crate::theme::{colors, text_size};
use crate::view::onboarding::{
    onboarding_illustration, onboarding_nav, onboarding_page, step_heading,
};

#[derive(PartialEq)]
pub struct OnboardingAccount;

impl Component for OnboardingAccount {
    fn render(&self) -> impl IntoElement {
        let account_query = use_current_account();
        let msa = use_microsoft_login();

        let account = try_default_account(&account_query);
        let has_account = account.is_some();

        let content = rect()
            .vertical()
            .width(Size::fill())
            .spacing(24.)
            .child(step_heading(
                t("oneclient.onboarding.account.title"),
                t("oneclient.onboarding.account.subtitle"),
            ))
            .child(match &account {
                Some(account) => account_preview(account).into_element(),
                // Qeezy: no approved Microsoft sign-in yet, Play goes through the official launcher
                None if !oneclient_common::constants::MSA_CONFIGURED => official_launcher_card().into_element(),
                None => {
                    let start = msa.clone();
                    sign_in_card(msa.pending, msa.error.clone(), move |_| start.start())
                        .into_element()
                }
            })
            .into_element();

        let page = onboarding_page(
            onboarding_illustration(IconType::OnboardingAccount),
            content,
            onboarding_nav(
                Some(Route::OnboardingLanguage {}),
                Route::OnboardingBundlePicker {},
                has_account || !oneclient_common::constants::MSA_CONFIGURED,
            ),
        );

        rect()
            .width(Size::fill())
            .height(Size::fill())
            .child(page)
            .maybe_child(msa.popup())
    }
}

fn account_preview(account: &MinecraftAccount) -> impl IntoElement {
    rect()
        .horizontal()
        .width(Size::fill())
        .spacing(24.)
        .child(
            rect()
                .horizontal()
                .spacing(12.)
                .cross_align(Alignment::Center)
                .child(
                    Avatar::new(account.id.to_string())
                        .width(Size::px(48.))
                        .height(Size::px(48.)),
                )
                .child(
                    rect()
                        .vertical()
                        .spacing(4.)
                        .child(
                            label()
                                .text(account.username.clone())
                                .font_size(text_size(16.))
                                .font_weight(FontWeight::SEMI_BOLD)
                                .color(colors::fg_primary()),
                        )
                        .child(
                            label()
                                .text(account.id.to_string())
                                .font_size(text_size(12.))
                                .color(colors::fg_secondary()),
                        ),
                ),
        )
        .into_element()
}

/// Qeezy: shown instead of the sign-in button until Mojang approves Qeezy Launcher.
fn official_launcher_card() -> impl IntoElement {
    rect()
        .vertical()
        .spacing(8.)
        .width(Size::fill())
        .child(
            rect()
                .horizontal()
                .spacing(8.)
                .cross_align(Alignment::Center)
                .child(Icon::new(IconType::InfoCircle).size(16.).color(colors::brand()))
                .child(
                    label()
                        .text("You play through the official Minecraft Launcher for now")
                        .font_size(text_size(16.))
                        .font_weight(FontWeight::SEMI_BOLD)
                        .color(colors::fg_primary()),
                ),
        )
        .child(
            label()
                .text(
                    "Qeezy Launcher's own Microsoft sign-in is waiting for Mojang's approval. Until then, \
                     Play sets everything up here, adds a \"Qeezy\" profile to the official Minecraft \
                     Launcher and opens it. You're already signed in there, so just press Play.",
                )
                .font_size(text_size(13.))
                .color(colors::fg_secondary()),
        )
        .into_element()
}

fn sign_in_card(
    pending: bool,
    error: Option<String>,
    on_add: impl FnMut(Event<PressEventData>) + 'static,
) -> impl IntoElement {
    rect()
        .vertical()
        .spacing(12.)
        .cross_align(Alignment::Start)
        .child(
            Button::new()
                .primary()
                .large()
                .enabled(!pending)
                .on_press(on_add)
                .child(Icon::new(IconType::Globe01).size(16.))
                .text(if pending {
                    t("oneclient.onboarding.account.signing_in")
                } else {
                    t("oneclient.onboarding.account.add")
                }),
        )
        .maybe_child(error.map(|message| {
            rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(6.)
                .child(
                    Icon::new(IconType::AlertTriangle)
                        .size(13.)
                        .color(colors::danger()),
                )
                .child(
                    label()
                        .text(message)
                        .font_size(text_size(12.))
                        .color(colors::danger()),
                )
                .into_element()
        }))
        .into_element()
}
