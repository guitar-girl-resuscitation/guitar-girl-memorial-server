use ggfm_protocol::{RpcClass, RpcName};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DomainHandler {
    Bootstrap,
    Account,
    Save,
    Catalogue,
    Purchase,
    Mail,
    Achievement,
    Follower,
    Pass,
    ChapterThree,
    Event,
    Telemetry,
    Compatibility,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandlerSpec {
    pub domain: DomainHandler,
    pub mutating: bool,
}

/// Every gameplay RPC is deliberately named here. New protocol evidence must
/// not silently inherit a generic state-changing implementation.
pub fn active_handler(name: RpcName) -> Option<HandlerSpec> {
    use DomainHandler::*;
    let (domain, mutating) = match name {
        RpcName::Init
        | RpcName::Main
        | RpcName::DefaultSettingList
        | RpcName::GetServerTime
        | RpcName::GetUpdateTime => (Bootstrap, false),
        RpcName::UserJoin => (Account, true),
        RpcName::UserLogin | RpcName::GetChoiceUser => (Account, false),
        RpcName::ChangeUser | RpcName::UserDel => (Account, true),
        RpcName::UserLoad | RpcName::LastSaveTime => (Save, false),
        RpcName::UserSave => (Save, true),
        RpcName::AvatarList
        | RpcName::TitleList
        | RpcName::UserTitleList
        | RpcName::GetCollection
        | RpcName::GetUserCollection
        | RpcName::GetVarietyStore
        | RpcName::GetGameDataList => (Catalogue, false),
        RpcName::BuyCheck | RpcName::CheckBuyShop | RpcName::CheckPurchased => (Purchase, false),
        RpcName::BuyContents
        | RpcName::BuyMusic
        | RpcName::BuyPackage
        | RpcName::BuyShop
        | RpcName::BuyVarietyStore
        | RpcName::UserPurchaseErrorLog => (Purchase, true),
        RpcName::GetPost | RpcName::GetPostTime => (Mail, false),
        RpcName::ProvidePost | RpcName::ReadPost | RpcName::DeletePost => (Mail, true),
        RpcName::GetAllPoint | RpcName::GetReviewPoint => (Achievement, false),
        RpcName::MusicPointReview => (Achievement, true),
        RpcName::UpdateAchievement
        | RpcName::UpdateAvatar
        | RpcName::UpdateScore
        | RpcName::SetReviewPopup => (Achievement, true),
        RpcName::SetFollowerProfileGift
        | RpcName::SetFollowerQuestInfinite
        | RpcName::SetUserFollowerProfileReward
        | RpcName::SetUserFollowerQuest => (Follower, true),
        RpcName::SetPassReward | RpcName::SetSubscribe => (Pass, true),
        RpcName::GetChThird => (ChapterThree, false),
        RpcName::ChThirdStage | RpcName::ChThirdStreamStage | RpcName::GetChThirdStarReward => {
            (ChapterThree, true)
        }
        RpcName::GetEventRewardList | RpcName::GetLocalPush | RpcName::GetSamSeckList => {
            (Event, false)
        }
        RpcName::GetSamSeckReward => (Event, true),
        RpcName::GetMusicReward => (Event, true),
        RpcName::PaidEventPoint
        | RpcName::SetAdReward
        | RpcName::SetAttendance
        | RpcName::SetBookmark
        | RpcName::SetEventReward
        | RpcName::SetGameReward
        | RpcName::SetSelectReward
        | RpcName::SetTutorial
        | RpcName::SetTutorialNew => (Event, true),
        RpcName::SetUserGP => (Save, true),
        RpcName::UpdateNickName => (Account, true),
        RpcName::UpdateUserTitle => (Achievement, true),
        RpcName::CompleteLog => (Telemetry, false),
        _ => return None,
    };
    Some(HandlerSpec { domain, mutating })
}

pub fn handler_for(name: RpcName) -> Option<HandlerSpec> {
    match name.class() {
        RpcClass::Active => active_handler(name),
        RpcClass::TransportOnly | RpcClass::DtoOnly => Some(HandlerSpec {
            domain: DomainHandler::Compatibility,
            mutating: false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_active_rpcs_have_an_explicit_handler() {
        let active: Vec<_> = RpcName::ALL
            .into_iter()
            .filter(|call| call.class() == RpcClass::Active)
            .collect();
        assert_eq!(active.len(), 69);
        for call in active {
            assert!(active_handler(call).is_some(), "{}", call.as_str());
        }
    }

    #[test]
    fn compatibility_handlers_cannot_mutate() {
        for call in RpcName::ALL
            .into_iter()
            .filter(|call| call.class() != RpcClass::Active)
        {
            assert!(!handler_for(call).unwrap().mutating, "{}", call.as_str());
        }
    }

    #[test]
    fn implementation_registry_matches_active_execution_boundary() {
        let implemented = RpcName::ALL
            .into_iter()
            .filter(|call| call.class() == RpcClass::Active)
            .filter(|call| crate::gameplay::is_implemented(*call))
            .count();
        assert_eq!(implemented, 69);
    }

    #[test]
    fn tested_rust_baseline_commands_are_never_classified_read_only() {
        // These names are the state-changing RPCs exercised by the tested
        // reborn-server baseline. Keep this list explicit: response-shape
        // registration alone must never silently downgrade a command to a
        // query during the architectural rewrite.
        const COMMANDS: &[RpcName] = &[
            RpcName::BuyContents,
            RpcName::BuyMusic,
            RpcName::BuyPackage,
            RpcName::BuyShop,
            RpcName::BuyVarietyStore,
            RpcName::ChThirdStage,
            RpcName::ChThirdStreamStage,
            RpcName::DeletePost,
            RpcName::GetChThirdStarReward,
            RpcName::GetMusicReward,
            RpcName::GetSamSeckReward,
            RpcName::MusicPointReview,
            RpcName::PaidEventPoint,
            RpcName::ProvidePost,
            RpcName::ReadPost,
            RpcName::SetAdReward,
            RpcName::SetAttendance,
            RpcName::SetBookmark,
            RpcName::SetEventReward,
            RpcName::SetFollowerProfileGift,
            RpcName::SetFollowerQuestInfinite,
            RpcName::SetGameReward,
            RpcName::SetPassReward,
            RpcName::SetSelectReward,
            RpcName::SetSubscribe,
            RpcName::SetTutorial,
            RpcName::SetTutorialNew,
            RpcName::SetUserGP,
            RpcName::SetUserFollowerProfileReward,
            RpcName::SetUserFollowerQuest,
            RpcName::UpdateAchievement,
            RpcName::UpdateAvatar,
            RpcName::UpdateNickName,
            RpcName::UpdateScore,
            RpcName::UpdateUserTitle,
            RpcName::UserSave,
        ];
        for command in COMMANDS {
            assert!(
                handler_for(*command).is_some_and(|handler| handler.mutating),
                "{} must retain tested Rust command semantics",
                command.as_str()
            );
        }
    }
}
