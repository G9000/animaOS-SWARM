import styles from './app.module.css';

export function App() {
  return (
    <div className={styles.shell}>
      <header className={styles.header}>
        <span className={styles.kicker}>animaOS</span>
        <h1 className={styles.title}>Control Grid</h1>
      </header>

      <main className={styles.main}>
        <p className={styles.placeholder}>Ready to build.</p>
      </main>
    </div>
  );
}

export default App;
