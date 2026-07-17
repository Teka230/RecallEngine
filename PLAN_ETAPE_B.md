# Upgrade de l'interface TUI (Étape B)

L'objectif de cette étape est de doter `recall browse` d'une interface riche, fluide et interactive semblable à CTK, en ajoutant le support de la souris, des layouts responsifs, la navigation au clavier (Vim-like/raccourcis) et un véritable champ de saisie de texte.

## User Review Required

> [!IMPORTANT]
> **Remplacement de la dépendance `tui-textarea` par `ratatui-textarea`** :
> Notre projet utilise `ratatui = "0.30"`. L'ancienne bibliothèque `tui-textarea 0.7.0` repose sur `ratatui 0.29`, ce qui causerait une duplication de dépendances et des erreurs de compatibilité de traits (car `Frame` en 0.29 n'est pas le même qu'en 0.30).  
> **Décision proposée :** J'ai validé localement que le fork officiel `ratatui-textarea = "0.9.2"` compile parfaitement avec notre projet. C'est donc cette crate moderne que nous utiliserons.

> [!NOTE]
> **Fluid sidebars & Layouts** :
> Les barres latérales "fluides" nécessiteront de refondre le `ui::render` avec de vrais `Layout::horizontal` et `Layout::vertical` en définissant des tailles proportionnelles (ex: `Constraint::Percentage`) et des largeurs fixes pour les colonnes, plutôt que les contraintes actuelles.

## Open Questions

Aucune question bloquante, le périmètre est clair.

## Proposed Changes

### Configuration du projet (Cargo.toml)

#### [MODIFY] Cargo.toml
- Ajouter la dépendance `ratatui-textarea = "0.9.2"` à la place de `tui-textarea`.

---

### Gestion d'État et des Focus

#### [MODIFY] src/tui/state.rs
- Ajouter une énumération `AppMode` ou `FocusPane` (par ex. `Sidebar`, `Chat`, `Input`) pour savoir quelle partie de l'interface est active (et reçoit donc les événements de frappe clavier).
- Ajouter le gestionnaire d'input : une instance de `TextArea<'a>` issue de `ratatui-textarea` stockée dans `App` pour le champ de recherche.
- Ajouter des `ListState` ou autres éléments d'état visuel propres à Ratatui pour que la souris ou les touches directionnelles puissent faire défiler les conversations indépendamment de l'arbre global, le cas échéant.

---

### Moteur Événementiel et Souris (Input Routing)

#### [MODIFY] src/tui/events.rs
- Activer la capture de la souris dans `crossterm` via `EnableMouseCapture` au lancement, et `DisableMouseCapture` en quittant.
- Gérer l'événement `Event::Mouse(MouseEvent)` en plus de `Event::Key`.
- **Hit-testing** : Lorsqu'un clic souris est détecté, calculer sa position `(x, y)` et déterminer quelle zone (Sidebar, Chat, Input) a été cliquée afin de modifier le `FocusPane`.
- Gérer le défilement souris (`ScrollUp`, `ScrollDown`) pour naviguer dans la vue active (chat ou historique).
- Déléguer les frappes clavier au `TextArea` si le focus est sur la zone de saisie `Input`.

---

### Refonte du Rendu Visuel (Layouts CTK)

#### [MODIFY] src/tui/ui.rs
- **Layout Global** : Diviser l'écran en un layout principal à 3 colonnes/parties :
  1. Panneau latéral gauche (liste des discussions / threads).
  2. Panneau central (historique des messages du thread courant).
  3. Barre de recherche (en bas ou overlay) gérée par `TextArea`.
- **Hitboxes** : Sauvegarder les rectangles (`Rect`) calculés par le `Layout` dans `App` ou les retourner afin de pouvoir faire le Hit-testing dans `events.rs`.
- **Thématisation** : Améliorer les bordures et les couleurs (`Color::Blue`, `Modifier::BOLD`) pour indiquer visuellement quel panneau a actuellement le *Focus*.
- Remplacer le rendu du paragraphe de recherche manuel par le `textarea.widget()`.

## Verification Plan

### Manual Verification
1. **Souris** : L'utilisateur devra lancer `cargo run -- browse` et vérifier qu'il peut cliquer sur le panneau latéral pour changer le focus, et scroller avec la molette de la souris.
2. **Saisie texte** : Cliquer ou naviguer vers le champ de recherche, y taper du texte, utiliser les flèches du clavier, s'assurer que c'est fluide et qu'il n'y a aucun ralentissement.
3. **Clavier** : S'assurer qu'on peut quitter avec `Esc` ou basculer les focus avec une touche (ex: `Tab`).
4. **Visuel** : Confirmer que l'interface ressemble davantage aux interfaces riches (comme CTK) avec des bordures pleines, colorées et un agencement multi-panneaux au lieu du mode "terminal de base".
